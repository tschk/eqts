use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, FnArg, GenericArgument, ItemFn, Pat, PathArguments, ReturnType,
    Type, parse_macro_input,
};

#[proc_macro_attribute]
pub fn export(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return error(
            proc_macro2::Span::call_site(),
            "eqts::export takes no arguments",
        );
    }
    let function = parse_macro_input!(input as ItemFn);
    expand_export(&function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(Record)]
pub fn record(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_record(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(Enum)]
pub fn unit_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_enum(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn error(span: proc_macro2::Span, message: &str) -> TokenStream {
    syn::Error::new(span, message).into_compile_error().into()
}

fn expand_export(function: &ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    validate_function(function)?;
    let name = &function.sig.ident;
    let wrapper = format_ident!("eqts_{name}");
    let arguments = function
        .sig
        .inputs
        .iter()
        .map(|argument| match argument {
            FnArg::Typed(argument) => argument,
            FnArg::Receiver(_) => unreachable!("validated argument"),
        })
        .collect::<Vec<_>>();
    let typed = arguments
        .iter()
        .map(|argument| type_kind(&argument.ty))
        .collect::<syn::Result<Vec<_>>>()?;
    let result = match &function.sig.output {
        ReturnType::Default => TypeKind::Void,
        ReturnType::Type(_, ty) => type_kind(ty)?,
    };
    let all_scalar = typed.iter().all(TypeKind::is_scalar) && result.is_scalar();
    if all_scalar {
        expand_scalar(function, name, &wrapper, &arguments, &typed, &result)
    } else {
        expand_json(function, name, &wrapper, &arguments, &typed, &result)
    }
}

fn expand_scalar(
    function: &ItemFn,
    name: &syn::Ident,
    wrapper: &syn::Ident,
    arguments: &[&syn::PatType],
    typed: &[TypeKind],
    result: &TypeKind,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut parameters = Vec::new();
    let mut inputs = Vec::new();
    let mut conversions = Vec::new();
    let mut calls = Vec::new();
    for (argument, kind) in arguments.iter().zip(typed) {
        let ident = plain_ident(argument)?;
        let abi_ident = format_ident!("__eqts_{ident}");
        let abi_ty = kind.abi_type();
        let ty = kind.schema();
        parameters.push(quote!(::eqts::Parameter { name: stringify!(#ident), ty: #ty }));
        inputs.push(quote!(#abi_ident: #abi_ty));
        calls.push(ident);
        conversions.push(if kind.is_bool() {
            quote!(let #ident = #abi_ident != 0;)
        } else {
            quote!(let #ident = #abi_ident;)
        });
    }
    let result_schema = result.schema();
    let (output, invoke) = if matches!(result, TypeKind::Void) {
        (
            quote!(),
            quote! { match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| { #(#conversions)* #name(#(#calls),*); })) { Ok(()) => ::eqts::ABI_OK, Err(_) => ::eqts::ABI_PANIC } },
        )
    } else {
        let abi_ty = result.abi_type();
        let conversion = if result.is_bool() {
            quote!(u8::from(value))
        } else {
            quote!(value)
        };
        (
            quote!(__eqts_output: *mut #abi_ty),
            quote! { match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| { #(#conversions)* #name(#(#calls),*) })) { Ok(value) => unsafe { ::eqts::__private::write_output(__eqts_output, #conversion) }, Err(_) => ::eqts::ABI_PANIC } },
        )
    };
    Ok(registration(
        function,
        name,
        wrapper,
        &quote!(::eqts::Abi::Scalar),
        &parameters,
        &result_schema,
        &quote! {
            #[unsafe(no_mangle)] pub unsafe extern "C" fn #wrapper(#(#inputs,)* #output) -> ::std::ffi::c_int { #invoke }
        },
    ))
}

fn expand_json(
    function: &ItemFn,
    name: &syn::Ident,
    wrapper: &syn::Ident,
    arguments: &[&syn::PatType],
    typed: &[TypeKind],
    result: &TypeKind,
) -> syn::Result<proc_macro2::TokenStream> {
    let mut parameters = Vec::new();
    let mut conversions = Vec::new();
    let mut calls = Vec::new();
    for (index, (argument, kind)) in arguments.iter().zip(typed).enumerate() {
        let ident = plain_ident(argument)?;
        let ty = &argument.ty;
        let schema = kind.schema();
        let index = syn::Index::from(index);
        parameters.push(quote!(::eqts::Parameter { name: stringify!(#ident), ty: #schema }));
        calls.push(ident);
        conversions.push(
            quote!(let #ident = <#ty as ::eqts::EqtsValue>::from_json(values[#index].take()).map_err(|error| (::eqts::ABI_INVALID_INPUT, error))?;),
        );
    }
    let count = arguments.len();
    let result_ty = match &function.sig.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => quote!(#ty),
    };
    let result_schema = result.schema();
    Ok(registration(
        function,
        name,
        wrapper,
        &quote!(::eqts::Abi::Json),
        &parameters,
        &result_schema,
        &quote! {
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn #wrapper(__eqts_input: *const u8, __eqts_len: usize, __eqts_output: *mut ::eqts::OwnedBuffer) -> ::std::ffi::c_int {
                if __eqts_output.is_null() { return ::eqts::ABI_NULL_OUTPUT; }
                let operation = ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| -> ::std::result::Result<::std::vec::Vec<u8>, (i32, ::std::string::String)> {
                    if __eqts_input.is_null() && __eqts_len != 0 { return Err((::eqts::ABI_INVALID_INPUT, "null input".into())); }
                    let input = if __eqts_len == 0 { &[][..] } else { unsafe { ::std::slice::from_raw_parts(__eqts_input, __eqts_len) } };
                    let mut values: ::std::vec::Vec<::eqts::__private::Value> = ::eqts::__private::from_slice(input).map_err(|error| (::eqts::ABI_INVALID_INPUT, error.to_string()))?;
                    if values.len() != #count { return Err((::eqts::ABI_INVALID_INPUT, format!("expected {} arguments, got {}", #count, values.len()))); }
                    #(#conversions)*
                    let value: #result_ty = #name(#(#calls),*);
                    let value = <#result_ty as ::eqts::EqtsValue>::into_json(value).map_err(|error| (::eqts::ABI_ENCODE_ERROR, error))?;
                    ::eqts::__private::to_vec(&value).map_err(|error| (::eqts::ABI_ENCODE_ERROR, error.to_string()))
                }));
                match operation {
                    Ok(Ok(bytes)) => { unsafe { __eqts_output.write(::eqts::OwnedBuffer::from_bytes(bytes)) }; ::eqts::ABI_OK }
                    Ok(Err((status, error))) => { unsafe { __eqts_output.write(::eqts::OwnedBuffer::from_bytes(error.into_bytes())) }; status }
                    Err(_) => { unsafe { __eqts_output.write(::eqts::OwnedBuffer::empty()) }; ::eqts::ABI_PANIC }
                }
            }
        },
    ))
}

fn registration(
    function: &ItemFn,
    name: &syn::Ident,
    wrapper: &syn::Ident,
    abi: &proc_macro2::TokenStream,
    parameters: &[proc_macro2::TokenStream],
    result: &proc_macro2::TokenStream,
    generated: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! { #function #generated ::eqts::inventory::submit! { ::eqts::FunctionRegistration { describe: || ::eqts::Function { module: module_path!(), name: stringify!(#name), symbol: stringify!(#wrapper), abi: #abi, parameters: vec![#(#parameters),*], result: #result } } } }
}

fn expand_record(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    reject_generics(input)?;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "Record requires a struct"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "Record requires named fields",
        ));
    };
    let name = &input.ident;
    let mut schemas = Vec::new();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for field in &fields.named {
        if !matches!(field.vis, syn::Visibility::Public(_)) {
            return Err(syn::Error::new_spanned(
                &field.vis,
                "Record fields must be public",
            ));
        }
        let ident = field.ident.as_ref().expect("named");
        let ty = &field.ty;
        schemas.push(quote!(::eqts::Field { name: stringify!(#ident), ty: <#ty as ::eqts::EqtsValue>::schema() }));
        reads.push(quote!(#ident: <#ty as ::eqts::EqtsValue>::from_json(object.remove(stringify!(#ident)).ok_or_else(|| concat!("missing field ", stringify!(#ident)).to_string())?)?));
        writes.push(quote!(object.insert(stringify!(#ident).into(), <#ty as ::eqts::EqtsValue>::into_json(self.#ident)?);));
    }
    Ok(
        quote! { impl ::eqts::EqtsValue for #name { fn schema() -> ::eqts::Type { ::eqts::Type::Record { name: stringify!(#name), fields: vec![#(#schemas),*] } } fn from_json(value: ::eqts::__private::Value) -> Result<Self, String> { let ::eqts::__private::Value::Object(mut object) = value else { return Err("expected object".into()); }; Ok(Self { #(#reads),* }) } fn into_json(self) -> Result<::eqts::__private::Value, String> { let mut object = ::eqts::__private::Value::Object(Default::default()); let ::eqts::__private::Value::Object(ref mut object) = object else { unreachable!() }; #(#writes)* Ok(::eqts::__private::Value::Object(::std::mem::take(object))) } } },
    )
}

fn expand_enum(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    reject_generics(input)?;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "Enum requires an enum"));
    };
    let name = &input.ident;
    let mut names = Vec::new();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                &variant.fields,
                "Enum supports unit variants only",
            ));
        }
        let ident = &variant.ident;
        names.push(quote!(stringify!(#ident)));
        reads.push(quote!(stringify!(#ident) => Ok(Self::#ident)));
        writes.push(quote!(Self::#ident => stringify!(#ident)));
    }
    Ok(
        quote! { impl ::eqts::EqtsValue for #name { fn schema() -> ::eqts::Type { ::eqts::Type::Enum { name: stringify!(#name), variants: &[#(#names),*] } } fn from_json(value: ::eqts::__private::Value) -> Result<Self, String> { let ::eqts::__private::Value::String(value) = value else { return Err("expected enum string".into()); }; match value.as_str() { #(#reads,)* _ => Err(format!("unknown {} variant: {}", stringify!(#name), value)) } } fn into_json(self) -> Result<::eqts::__private::Value, String> { Ok(::eqts::__private::Value::String(match self { #(#writes),* }.into())) } } },
    )
}

fn validate_function(function: &ItemFn) -> syn::Result<()> {
    if !matches!(function.vis, syn::Visibility::Public(_)) {
        return Err(syn::Error::new_spanned(
            &function.vis,
            "eqts exports must be public",
        ));
    }
    if function.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            function.sig.asyncness,
            "eqts exports do not support async functions",
        ));
    }
    if function.sig.constness.is_some()
        || function.sig.unsafety.is_some()
        || function.sig.abi.is_some()
        || function.sig.variadic.is_some()
    {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "eqts exports require safe Rust functions",
        ));
    }
    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.generics,
            "eqts exports cannot be generic",
        ));
    }
    for argument in &function.sig.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "eqts exports must be free functions",
            ));
        };
        plain_ident(argument)?;
    }
    Ok(())
}
fn reject_generics(input: &DeriveInput) -> syn::Result<()> {
    if input.generics.params.is_empty() && input.generics.where_clause.is_none() {
        Ok(())
    } else {
        Err(syn::Error::new_spanned(
            &input.generics,
            "eqts derived types cannot be generic",
        ))
    }
}
fn plain_ident(argument: &syn::PatType) -> syn::Result<&syn::Ident> {
    let Pat::Ident(pattern) = argument.pat.as_ref() else {
        return Err(syn::Error::new_spanned(
            &argument.pat,
            "eqts parameters must use identifier patterns",
        ));
    };
    if pattern.by_ref.is_some() || pattern.subpat.is_some() {
        return Err(syn::Error::new_spanned(
            pattern,
            "eqts parameters must be plain identifiers",
        ));
    }
    Ok(&pattern.ident)
}

enum TypeKind {
    Void,
    Scalar(proc_macro2::Ident),
    String,
    Bytes,
    Vec(Box<TypeKind>),
    Option(Box<TypeKind>),
    Result(Box<TypeKind>, Box<TypeKind>),
    Named(Type),
}
impl TypeKind {
    fn is_scalar(&self) -> bool {
        matches!(self, Self::Void | Self::Scalar(_))
    }
    fn is_bool(&self) -> bool {
        matches!(self, Self::Scalar(value) if value == "Bool")
    }
    fn abi_type(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Scalar(value) if value == "Bool" => quote!(u8),
            Self::Scalar(value) => {
                let ident = format_ident!("{}", value.to_string().to_lowercase());
                quote!(#ident)
            }
            Self::Void => quote!(()),
            _ => unreachable!(),
        }
    }
    fn schema(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Void => quote!(::eqts::Type::Scalar {
                scalar: ::eqts::Scalar::Void
            }),
            Self::Scalar(value) => quote!(::eqts::Type::Scalar { scalar: ::eqts::Scalar::#value }),
            Self::String => quote!(::eqts::Type::String),
            Self::Bytes => quote!(::eqts::Type::Bytes),
            Self::Vec(value) => {
                let value = value.schema();
                quote!(::eqts::Type::Vec { value: Box::new(#value) })
            }
            Self::Option(value) => {
                let value = value.schema();
                quote!(::eqts::Type::Option { value: Box::new(#value) })
            }
            Self::Result(ok, error) => {
                let ok = ok.schema();
                let error = error.schema();
                quote!(::eqts::Type::Result { ok: Box::new(#ok), error: Box::new(#error) })
            }
            Self::Named(ty) => quote!(<#ty as ::eqts::EqtsValue>::schema()),
        }
    }
}
fn type_kind(ty: &Type) -> syn::Result<TypeKind> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(ty, "unsupported eqts type"));
    };
    if path.qself.is_some() {
        return Err(syn::Error::new_spanned(ty, "unsupported eqts type"));
    }
    let segment = path.path.segments.last().expect("path");
    let scalar = match segment.ident.to_string().as_str() {
        "bool" => Some("Bool"),
        "u8" => Some("U8"),
        "u16" => Some("U16"),
        "u32" => Some("U32"),
        "u64" => Some("U64"),
        "i8" => Some("I8"),
        "i16" => Some("I16"),
        "i32" => Some("I32"),
        "i64" => Some("I64"),
        "f32" => Some("F32"),
        "f64" => Some("F64"),
        _ => None,
    };
    if let Some(value) = scalar {
        return Ok(TypeKind::Scalar(format_ident!("{value}")));
    }
    if segment.ident == "String" {
        return Ok(TypeKind::String);
    }
    if segment.ident == "Vec" {
        let inner = one_argument(segment)?;
        if matches!(type_kind(inner)?,TypeKind::Scalar(ref value) if value=="U8") {
            return Ok(TypeKind::Bytes);
        }
        return Ok(TypeKind::Vec(Box::new(type_kind(inner)?)));
    }
    if segment.ident == "Option" {
        return Ok(TypeKind::Option(Box::new(type_kind(one_argument(
            segment,
        )?)?)));
    }
    if segment.ident == "Result" {
        let PathArguments::AngleBracketed(args) = &segment.arguments else {
            return Err(syn::Error::new_spanned(ty, "Result requires two types"));
        };
        let mut args = args.args.iter().filter_map(|arg| {
            if let GenericArgument::Type(ty) = arg {
                Some(ty)
            } else {
                None
            }
        });
        let ok = args
            .next()
            .ok_or_else(|| syn::Error::new_spanned(ty, "Result requires two types"))?;
        let error = args
            .next()
            .ok_or_else(|| syn::Error::new_spanned(ty, "Result requires two types"))?;
        if args.next().is_some() {
            return Err(syn::Error::new_spanned(ty, "Result requires two types"));
        }
        return Ok(TypeKind::Result(
            Box::new(type_kind(ok)?),
            Box::new(type_kind(error)?),
        ));
    }
    if matches!(segment.arguments, PathArguments::None) {
        return Ok(TypeKind::Named(ty.clone()));
    }
    Err(syn::Error::new_spanned(ty, "unsupported eqts type"))
}
fn one_argument(segment: &syn::PathSegment) -> syn::Result<&Type> {
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err(syn::Error::new_spanned(
            segment,
            "type requires one argument",
        ));
    };
    if args.args.len() != 1 {
        return Err(syn::Error::new_spanned(args, "type requires one argument"));
    }
    let Some(GenericArgument::Type(ty)) = args.args.first() else {
        return Err(syn::Error::new_spanned(
            args,
            "type requires one type argument",
        ));
    };
    Ok(ty)
}
