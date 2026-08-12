use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, FnArg, GenericArgument, ImplItem, ItemFn, ItemImpl, Pat,
    PathArguments, ReturnType, Type, parse_macro_input,
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

macro_rules! reactive_attribute {
    ($name:ident, $kind:ident) => {
        #[proc_macro_attribute]
        pub fn $name(args: TokenStream, input: TokenStream) -> TokenStream {
            let value = parse_macro_input!(args as Type);
            let function = parse_macro_input!(input as ItemFn);
            expand_reactive(&function, &value, &quote!(::eqts::ExportKind::$kind))
                .unwrap_or_else(syn::Error::into_compile_error)
                .into()
        }
    };
}

reactive_attribute!(object, Object);
reactive_attribute!(trait_export, Trait);
reactive_attribute!(async_export, Async);
reactive_attribute!(callback, Callback);
reactive_attribute!(stream, Stream);
reactive_attribute!(iterator, Iterator);

#[proc_macro_attribute]
pub fn methods(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    expand_methods(&item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_attribute]
pub fn method(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

fn expand_methods(item: &ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    if item.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            item,
            "methods requires an inherent impl",
        ));
    }
    let self_ty = &item.self_ty;
    let mut descriptors = Vec::new();
    let mut arms = Vec::new();
    let mut async_arms = Vec::new();
    for member in &item.items {
        let ImplItem::Fn(method) = member else {
            continue;
        };
        if !matches!(method.vis, syn::Visibility::Public(_)) {
            continue;
        }
        let asynchronous = method.sig.asyncness.is_some();
        let Some(FnArg::Receiver(receiver)) = method.sig.inputs.first() else {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "method requires a receiver",
            ));
        };
        let name = &method.sig.ident;
        let mutable = receiver.mutability.is_some();
        if asynchronous && mutable {
            return Err(syn::Error::new_spanned(
                receiver,
                "async methods cannot use &mut self",
            ));
        }
        let mut parameters = Vec::new();
        let mut reads = Vec::new();
        let mut calls = Vec::new();
        for (index, arg) in method.sig.inputs.iter().skip(1).enumerate() {
            let FnArg::Typed(arg) = arg else {
                unreachable!()
            };
            let ident = plain_ident(arg)?;
            let ty = &arg.ty;
            let schema = type_kind(ty)?.schema();
            let index = syn::Index::from(index);
            parameters.push(quote!(::eqts::Parameter{name:stringify!(#ident),ty:#schema}));
            reads.push(quote!(let #ident=<#ty as ::eqts::EqtsValue>::from_json(arguments[#index].take())?;));
            calls.push(ident);
        }
        let count = parameters.len();
        let (result_ty, result) = match &method.sig.output {
            ReturnType::Default => (
                quote!(()),
                quote!(::eqts::Type::Scalar{scalar:::eqts::Scalar::Void}),
            ),
            ReturnType::Type(_, ty) => (quote!(#ty), type_kind(ty)?.schema()),
        };
        descriptors.push(quote!(::eqts::Method{name:stringify!(#name),parameters:vec![#(#parameters),*],result:#result,mutable:#mutable,asynchronous:#asynchronous}));
        if asynchronous {
            async_arms.push(quote!(stringify!(#name)=>{if arguments.len()!=#count{return Err(format!("expected {} arguments, got {}",#count,arguments.len()));}#(#reads)*let snapshot=self.clone();Ok(::eqts::register_future(async move{snapshot.#name(#(#calls),*).await}))}));
        } else {
            arms.push(quote!(stringify!(#name)=>{if arguments.len()!=#count{return Err(format!("expected {} arguments, got {}",#count,arguments.len()));}#(#reads)*let value:#result_ty=self.#name(#(#calls),*);<#result_ty as ::eqts::EqtsValue>::into_json(value)}));
        }
    }
    let cleaned = item.items.iter().map(|member| {
        let mut member = member.clone();
        if let ImplItem::Fn(method) = &mut member {
            method.attrs.retain(|attr| !attr.path().is_ident("method"));
        }
        member
    });
    Ok(
        quote!(impl #self_ty{#(#cleaned)*}impl ::eqts::EqtsMethods for #self_ty{fn invoke(&mut self,method:&str,mut arguments:Vec<::eqts::__private::Value>)->Result<::eqts::__private::Value,String>{match method{#(#arms,)*_=>Err(format!("unknown method: {method}"))}}fn invoke_async(&mut self,method:&str,mut arguments:Vec<::eqts::__private::Value>)->Result<u64,String>{match method{#(#async_arms,)*_=>Err(format!("unknown async method: {method}"))}}}::eqts::inventory::submit!{::eqts::MethodRegistration{describe:||::eqts::MethodSet{rust_type:stringify!(#self_ty),methods:vec![#(#descriptors),*]}}}),
    )
}

#[expect(clippy::too_many_lines)]
fn expand_reactive(
    function: &ItemFn,
    value: &Type,
    kind: &proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let is_async = kind.to_string().ends_with("Async");
    let is_callback = kind.to_string().ends_with("Callback");
    if is_async {
        if function.sig.asyncness.is_none() {
            return Err(syn::Error::new_spanned(
                &function.sig,
                "async_export requires an async function",
            ));
        }
        if !matches!(function.vis, syn::Visibility::Public(_))
            || !function.sig.generics.params.is_empty()
        {
            return Err(syn::Error::new_spanned(
                &function.sig,
                "reactive exports must be public and non-generic",
            ));
        }
    } else {
        validate_function(function)?;
    }
    let all_arguments = function
        .sig
        .inputs
        .iter()
        .map(|argument| match argument {
            FnArg::Typed(argument) => argument,
            FnArg::Receiver(_) => unreachable!("validated"),
        })
        .collect::<Vec<_>>();
    let (callback_name, arguments) = if is_callback {
        let callback = all_arguments.first().ok_or_else(|| {
            syn::Error::new_spanned(
                &function.sig,
                "callback export requires Callback<T> as first parameter",
            )
        })?;
        (Some(plain_ident(callback)?), all_arguments[1..].to_vec())
    } else {
        (None, all_arguments)
    };
    let mut parameters = Vec::new();
    let mut conversions = Vec::new();
    let mut calls = Vec::new();
    let mut napi_inputs = Vec::new();
    let mut napi_values = Vec::new();
    let mut wasm_inputs = Vec::new();
    let mut wasm_values = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        let ident = plain_ident(argument)?;
        let ty = &argument.ty;
        let schema = type_kind(ty)?.schema();
        let index = syn::Index::from(index);
        let transport = format_ident!("__eqts_{ident}");
        parameters.push(quote!(::eqts::Parameter { name: stringify!(#ident), ty: #schema }));
        conversions.push(
            quote!(let #ident = <#ty as ::eqts::EqtsValue>::from_json(values[#index].take())?;),
        );
        calls.push(ident);
        napi_inputs.push(quote!(#transport: ::eqts::__private::Value));
        napi_values.push(quote!(#transport));
        wasm_inputs.push(quote!(#transport: ::eqts::wasm_bindgen::JsValue));
        wasm_values.push(quote!(::eqts::serde_wasm_bindgen::from_value(#transport).map_err(|error| error.to_string())?));
    }
    let count = arguments.len();
    let name = &function.sig.ident;
    let wrapper = format_ident!("eqts_{name}");
    let napi = format_ident!("__eqts_napi_{name}");
    let wasm = format_ident!("__eqts_wasm_{name}");
    let js_name = syn::LitStr::new(&camel_case(&name.to_string()), name.span());
    let construct = format_ident!("__eqts_construct_{name}");
    let is_object = kind.to_string().ends_with("Object") || kind.to_string().ends_with("Trait");
    let register = if is_async {
        quote!(::eqts::register_future(#name(#(#calls),*)))
    } else if let Some(callback) = callback_name {
        quote!(::eqts::register_callback::<#value>(|#callback| Box::new(#name(#callback, #(#calls),*))))
    } else if is_object {
        quote!(::eqts::register_object(#name(#(#calls),*)))
    } else {
        quote!(::eqts::register_reactive(#name(#(#calls),*)))
    };
    let kind_value = match kind.to_string().as_str() {
        kind_name if kind_name.ends_with("Stream") || kind_name.ends_with("Iterator") => {
            quote!(#kind { item: <#value as ::eqts::EqtsValue>::schema() })
        }
        kind_name if kind_name.ends_with("Callback") => {
            let callback = callback_name.expect("validated callback");
            quote!(#kind { value: <#value as ::eqts::EqtsValue>::schema(), callback_parameter: stringify!(#callback) })
        }
        _ => quote!(#kind { value: <#value as ::eqts::EqtsValue>::schema() }),
    };
    Ok(quote! {
        #function
        fn #construct(mut values: Vec<::eqts::__private::Value>) -> Result<u64, String> {
            if values.len() != #count { return Err(format!("expected {} arguments, got {}", #count, values.len())); }
            #(#conversions)*
            Ok(#register)
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #wrapper(input: *const u8, len: usize, output: *mut u64) -> i32 {
            if output.is_null() || (input.is_null() && len != 0) { return ::eqts::ABI_NULL_OUTPUT; }
            let input = if len == 0 { &b"[]"[..] } else { unsafe { std::slice::from_raw_parts(input, len) } };
            let values = match ::eqts::__private::from_slice(input) { Ok(values) => values, Err(_) => return ::eqts::ABI_INVALID_INPUT };
            match #construct(values) { Ok(handle) => unsafe { ::eqts::__private::write_output(output, handle) }, Err(_) => ::eqts::ABI_INVALID_INPUT }
        }
        #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
        #[::eqts::napi_derive::napi(js_name = #js_name)]
        fn #napi(#(#napi_inputs),*) -> ::eqts::napi::Result<::eqts::napi::bindgen_prelude::BigInt> { #construct(vec![#(#napi_values),*]).map(Into::into).map_err(::eqts::napi::Error::from_reason) }
        #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
        #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = #js_name)]
        pub fn #wasm(#(#wasm_inputs),*) -> Result<u64, ::eqts::wasm_bindgen::JsValue> { let values = (|| -> Result<_, String> { Ok(vec![#(#wasm_values),*]) })().map_err(|error| ::eqts::js_sys::Error::new(&error))?; #construct(values).map_err(|error| ::eqts::js_sys::Error::new(&error).into()) }
        ::eqts::inventory::submit! { ::eqts::FunctionRegistration { describe: || ::eqts::Function {
            module: module_path!(), name: stringify!(#name), symbol: stringify!(#wrapper), abi: ::eqts::Abi::Json,
            parameters: vec![#(#parameters),*], result: ::eqts::Type::Scalar { scalar: ::eqts::Scalar::U64 }, kind: #kind_value
        } } }
    })
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

#[expect(clippy::too_many_lines)]
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
    let napi_bridge = format_ident!("__eqts_napi_{name}");
    let wasm_bridge = format_ident!("__eqts_wasm_{name}");
    let js_name = syn::LitStr::new(&camel_case(&name.to_string()), name.span());
    let direct_inputs = arguments
        .iter()
        .map(|argument| {
            let ident = plain_ident(argument).expect("validated parameter");
            let ty = &argument.ty;
            quote!(#ident: #ty)
        })
        .collect::<Vec<_>>();
    let napi_inputs = arguments
        .iter()
        .zip(typed)
        .map(|(argument, kind)| {
            let ident = plain_ident(argument).expect("validated parameter");
            if kind.is_64_bit() {
                quote!(#ident: ::eqts::napi::bindgen_prelude::BigInt)
            } else {
                let ty = &argument.ty;
                quote!(#ident: #ty)
            }
        })
        .collect::<Vec<_>>();
    let napi_conversions = arguments.iter().zip(typed).map(|(argument, kind)| {
        let ident = plain_ident(argument).expect("validated parameter");
        let TypeKind::Scalar(scalar) = kind else { return quote!(); };
        if scalar == "U64" { quote!(let (_, #ident, lossless) = #ident.get_u64(); if !lossless { return Err(::eqts::napi::Error::from_reason("u64 BigInt is out of range")); }) }
        else if scalar == "I64" { quote!(let (#ident, lossless) = #ident.get_i64(); if !lossless { return Err(::eqts::napi::Error::from_reason("i64 BigInt is out of range")); }) }
        else { quote!() }
    }).collect::<Vec<_>>();
    let direct_calls = arguments
        .iter()
        .map(|argument| plain_ident(argument).expect("validated parameter"))
        .collect::<Vec<_>>();
    let direct_output = &function.sig.output;
    let rust_result_ty = match &function.sig.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => quote!(#ty),
    };
    let napi_output = if result.is_64_bit() {
        quote!(-> ::eqts::napi::Result<::eqts::napi::bindgen_prelude::BigInt>)
    } else {
        quote!(-> ::eqts::napi::Result<#rust_result_ty>)
    };
    let napi_call = if result.is_64_bit() {
        quote!(Ok(#name(#(#direct_calls),*).into()))
    } else {
        quote!(Ok(#name(#(#direct_calls),*)))
    };
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
            #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
            #[::eqts::napi_derive::napi(js_name = #js_name)]
            fn #napi_bridge(#(#napi_inputs),*) #napi_output { #(#napi_conversions)* #napi_call }

            #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
            #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = #js_name)]
            pub fn #wasm_bridge(#(#direct_inputs),*) #direct_output { #name(#(#direct_calls),*) }

            #[unsafe(no_mangle)]
            #[doc = "# Safety"]
            #[doc = "Every output pointer must be null or valid, aligned, and writable for its declared type."]
            pub unsafe extern "C" fn #wrapper(#(#inputs,)* #output) -> ::std::ffi::c_int { #invoke }
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
    let mut napi_inputs = Vec::new();
    let mut napi_values = Vec::new();
    let mut wasm_inputs = Vec::new();
    let mut wasm_values = Vec::new();
    for (index, (argument, kind)) in arguments.iter().zip(typed).enumerate() {
        let ident = plain_ident(argument)?;
        let ty = &argument.ty;
        let schema = kind.schema();
        let index = syn::Index::from(index);
        parameters.push(quote!(::eqts::Parameter { name: stringify!(#ident), ty: #schema }));
        calls.push(ident);
        let transport_ident = format_ident!("__eqts_{ident}");
        napi_inputs.push(quote!(#transport_ident: ::eqts::__private::Value));
        napi_values.push(quote!(#transport_ident));
        wasm_inputs.push(quote!(#transport_ident: ::eqts::wasm_bindgen::JsValue));
        wasm_values.push(quote!(::eqts::serde_wasm_bindgen::from_value(#transport_ident).map_err(|error| error.to_string())?));
        conversions.push(
            quote!(let #ident = <#ty as ::eqts::EqtsValue>::from_json(values[#index].take())?;),
        );
    }
    let count = arguments.len();
    let result_ty = match &function.sig.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => quote!(#ty),
    };
    let result_schema = result.schema();
    let bridge = format_ident!("__eqts_bridge_{name}");
    let transport = format_ident!("__eqts_transport_{name}");
    let napi_bridge = format_ident!("__eqts_napi_{name}");
    let wasm_bridge = format_ident!("__eqts_wasm_{name}");
    let js_name = syn::LitStr::new(&camel_case(&name.to_string()), name.span());
    let transport_result = transport_result(result);
    Ok(registration(
        function,
        name,
        wrapper,
        &quote!(::eqts::Abi::Json),
        &parameters,
        &result_schema,
        &quote! {
            fn #bridge(mut values: ::std::vec::Vec<::eqts::__private::Value>) -> ::std::result::Result<::eqts::__private::Value, ::std::string::String> {
                if values.len() != #count { return Err(format!("expected {} arguments, got {}", #count, values.len())); }
                #(#conversions)*
                let value: #result_ty = #name(#(#calls),*);
                let value = <#result_ty as ::eqts::EqtsValue>::into_json(value)?;
                Ok(value)
            }

            fn #transport(value: ::eqts::__private::Value) -> ::std::result::Result<::eqts::__private::Value, ::std::string::String> {
                #transport_result
            }

            #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
            #[::eqts::napi_derive::napi(js_name = #js_name)]
            fn #napi_bridge(#(#napi_inputs),*) -> ::eqts::napi::Result<::eqts::__private::Value> {
                let value = #bridge(vec![#(#napi_values),*]).and_then(#transport);
                value.map_err(::eqts::napi::Error::from_reason)
            }

            #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
            #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = #js_name)]
            pub fn #wasm_bridge(#(#wasm_inputs),*) -> ::std::result::Result<::eqts::wasm_bindgen::JsValue, ::eqts::wasm_bindgen::JsValue> {
                let values = (|| -> ::std::result::Result<_, ::std::string::String> { Ok(vec![#(#wasm_values),*]) })()
                    .map_err(|error| ::eqts::js_sys::Error::new(&error))?;
                let value = #bridge(values).and_then(#transport).map_err(|error| ::eqts::js_sys::Error::new(&error))?;
                let serializer = ::eqts::serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
                ::eqts::serde::Serialize::serialize(&value, &serializer).map_err(|error| ::eqts::js_sys::Error::new(&error.to_string()).into())
            }

            #[unsafe(no_mangle)]
            #[doc = "# Safety"]
            #[doc = "Input must be null with zero length or readable for `len` bytes; output must be valid, aligned, and writable."]
            pub unsafe extern "C" fn #wrapper(__eqts_input: *const u8, __eqts_len: usize, __eqts_output: *mut ::eqts::OwnedBuffer) -> ::std::ffi::c_int {
                if __eqts_output.is_null() { return ::eqts::ABI_NULL_OUTPUT; }
                let operation = ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| -> ::std::result::Result<::std::vec::Vec<u8>, (i32, ::std::string::String)> {
                    if __eqts_input.is_null() && __eqts_len != 0 { return Err((::eqts::ABI_INVALID_INPUT, "null input".into())); }
                    let input = if __eqts_len == 0 { &[][..] } else { unsafe { ::std::slice::from_raw_parts(__eqts_input, __eqts_len) } };
                    let values: ::std::vec::Vec<::eqts::__private::Value> = ::eqts::__private::from_slice(input).map_err(|error| (::eqts::ABI_INVALID_INPUT, error.to_string()))?;
                    let value = #bridge(values).map_err(|error| (::eqts::ABI_INVALID_INPUT, error))?;
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

fn transport_result(result: &TypeKind) -> proc_macro2::TokenStream {
    if matches!(result, TypeKind::Result(_, _)) {
        quote! {
            let ::eqts::__private::Value::Object(mut object) = value else { return Err("invalid Result encoding".into()); };
            if let Some(error) = object.remove("error") {
                return Err(match error {
                    ::eqts::__private::Value::String(message) => message,
                    value => value.to_string(),
                });
            }
            object.remove("ok").ok_or_else(|| "invalid Result encoding".into())
        }
    } else {
        quote!(Ok(value))
    }
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
    quote! { #function #generated ::eqts::inventory::submit! { ::eqts::FunctionRegistration { describe: || ::eqts::Function { module: module_path!(), name: stringify!(#name), symbol: stringify!(#wrapper), abi: #abi, parameters: vec![#(#parameters),*], result: #result, kind: ::eqts::ExportKind::Function } } } }
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
    fn is_64_bit(&self) -> bool {
        matches!(self, Self::Scalar(value) if value == "U64" || value == "I64")
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
#[expect(clippy::too_many_lines)]
fn type_kind(ty: &Type) -> syn::Result<TypeKind> {
    match ty {
        Type::BareFn(_) => {
            return Err(syn::Error::new_spanned(
                ty,
                "eqts callbacks are not supported across every transport",
            ));
        }
        Type::ImplTrait(_) => {
            return Err(syn::Error::new_spanned(
                ty,
                "eqts trait, iterator, and stream exports are not supported across every transport",
            ));
        }
        Type::TraitObject(_) => {
            return Err(syn::Error::new_spanned(
                ty,
                "eqts trait objects are not supported across every transport",
            ));
        }
        Type::Reference(_) => {
            return Err(syn::Error::new_spanned(
                ty,
                "eqts borrowed and stateful object values are not supported across every transport",
            ));
        }
        _ => {}
    }
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
    if segment.ident == "Box" {
        type_kind(one_argument(segment)?)?;
        return Err(syn::Error::new_spanned(
            ty,
            "eqts owned boxes are not supported across every transport",
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

fn camel_case(value: &str) -> String {
    let mut uppercase = false;
    value
        .chars()
        .filter_map(|character| {
            if character == '_' {
                uppercase = true;
                None
            } else if uppercase {
                uppercase = false;
                Some(character.to_ascii_uppercase())
            } else {
                Some(character)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_bridge_uses_plain_wasm_objects_and_unquoted_string_errors() {
        let function: ItemFn = syn::parse_quote! {
            pub fn make_value(value: String) -> Result<String, String> { Ok(value) }
        };
        let output = expand_export(&function)
            .expect("rich export must expand")
            .to_string();
        assert!(output.contains("serialize_maps_as_objects"));
        assert!(output.contains("Value :: String (message) => message"));
    }
}
