use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat, ReturnType, Type, parse_macro_input};

#[proc_macro_attribute]
pub fn export(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "eqts::export takes no arguments",
        )
        .into_compile_error()
        .into();
    }
    let function = parse_macro_input!(input as ItemFn);
    match expand_export(&function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_export(function: &ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    validate_function(function)?;

    let name = &function.sig.ident;
    let wrapper = format_ident!("eqts_{name}");
    let mut parameters = Vec::new();
    let mut wrapper_inputs = Vec::new();
    let mut conversions = Vec::new();
    let mut call_args = Vec::new();

    for argument in &function.sig.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "eqts exports must be free functions",
            ));
        };
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
        let scalar = scalar(&argument.ty)?;
        let ident = &pattern.ident;
        let abi_ident = format_ident!("__eqts_{ident}");
        let abi_ty = scalar.abi_type();
        let conversion = scalar.input_conversion(&abi_ident);
        parameters.push(quote! {
            ::eqts::Parameter {
                name: stringify!(#ident),
                scalar: #scalar,
            }
        });
        wrapper_inputs.push(quote!(#abi_ident: #abi_ty));
        conversions.push(quote!(let #ident = #conversion;));
        call_args.push(ident);
    }

    let (result, output, invoke) = match &function.sig.output {
        ReturnType::Default => (
            quote!(::eqts::Scalar::Void),
            quote!(),
            quote! {
                match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| {
                    #(#conversions)*
                    #name(#(#call_args),*);
                })) {
                    Ok(()) => ::eqts::ABI_OK,
                    Err(_) => ::eqts::ABI_PANIC,
                }
            },
        ),
        ReturnType::Type(_, ty) => {
            let scalar = scalar(ty)?;
            let abi_ty = scalar.abi_type();
            let output_conversion = scalar.output_conversion();
            (
                quote!(#scalar),
                quote!(__eqts_output: *mut #abi_ty),
                quote! {
                    match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| {
                        #(#conversions)*
                        #name(#(#call_args),*)
                    })) {
                        Ok(value) => {
                            let value = #output_conversion;
                            unsafe { ::eqts::__private::write_output(__eqts_output, value) }
                        }
                        Err(_) => ::eqts::ABI_PANIC,
                    }
                },
            )
        }
    };

    Ok(quote! {
        #[cfg_attr(all(feature = "node-napi", not(feature = "wasm")), ::eqts::napi_derive::napi)]
        #[cfg_attr(all(feature = "wasm", not(feature = "node-napi")), ::eqts::wasm_bindgen::prelude::wasm_bindgen)]
        #function

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #wrapper(#(#wrapper_inputs,)* #output) -> ::std::ffi::c_int {
            #invoke
        }

        ::eqts::inventory::submit! {
            ::eqts::Function {
                module: module_path!(),
                name: stringify!(#name),
                symbol: concat!("eqts_", stringify!(#name)),
                parameters: &[#(#parameters),*],
                result: #result,
            }
        }
    })
}

fn validate_function(function: &ItemFn) -> syn::Result<()> {
    if !matches!(function.vis, syn::Visibility::Public(_)) {
        return Err(syn::Error::new_spanned(
            &function.vis,
            "eqts exports must be public",
        ));
    }
    if function.sig.constness.is_some() {
        return Err(syn::Error::new_spanned(
            function.sig.constness,
            "eqts exports cannot be const",
        ));
    }
    if function.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            function.sig.asyncness,
            "eqts exports do not support async functions",
        ));
    }
    if function.sig.unsafety.is_some() {
        return Err(syn::Error::new_spanned(
            function.sig.unsafety,
            "eqts exports cannot be unsafe",
        ));
    }
    if function.sig.abi.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.abi,
            "eqts exports cannot declare an ABI",
        ));
    }
    if function.sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.variadic,
            "eqts exports cannot be variadic",
        ));
    }
    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.generics,
            "eqts exports cannot be generic",
        ));
    }
    Ok(())
}

struct ScalarType {
    variant: proc_macro2::Ident,
    abi: proc_macro2::TokenStream,
    boolean: bool,
}

impl quote::ToTokens for ScalarType {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let variant = &self.variant;
        tokens.extend(quote!(::eqts::Scalar::#variant));
    }
}

impl ScalarType {
    fn abi_type(&self) -> &proc_macro2::TokenStream {
        &self.abi
    }

    fn input_conversion(&self, input: &syn::Ident) -> proc_macro2::TokenStream {
        if self.boolean {
            quote!(#input != 0)
        } else {
            quote!(#input)
        }
    }

    fn output_conversion(&self) -> proc_macro2::TokenStream {
        if self.boolean {
            quote!(u8::from(value))
        } else {
            quote!(value)
        }
    }
}

fn scalar(ty: &Type) -> syn::Result<ScalarType> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "eqts exports require scalar value types",
        ));
    };
    if path.qself.is_some() {
        return Err(syn::Error::new_spanned(
            ty,
            "eqts exports require primitive types",
        ));
    }
    let Some(ident) = path.path.get_ident() else {
        return Err(syn::Error::new_spanned(
            ty,
            "eqts exports require primitive types",
        ));
    };
    let (variant, abi, boolean) = match ident.to_string().as_str() {
        "bool" => (format_ident!("Bool"), quote!(u8), true),
        "u8" => (format_ident!("U8"), quote!(u8), false),
        "u16" => (format_ident!("U16"), quote!(u16), false),
        "u32" => (format_ident!("U32"), quote!(u32), false),
        "u64" => (format_ident!("U64"), quote!(u64), false),
        "i8" => (format_ident!("I8"), quote!(i8), false),
        "i16" => (format_ident!("I16"), quote!(i16), false),
        "i32" => (format_ident!("I32"), quote!(i32), false),
        "i64" => (format_ident!("I64"), quote!(i64), false),
        "f32" => (format_ident!("F32"), quote!(f32), false),
        "f64" => (format_ident!("F64"), quote!(f64), false),
        _ => {
            return Err(syn::Error::new_spanned(
                ty,
                "unsupported eqts type; use bool or a fixed-width numeric scalar",
            ));
        }
    };
    Ok(ScalarType {
        variant,
        abi,
        boolean,
    })
}
