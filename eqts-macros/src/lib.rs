use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat, ReturnType, Type, parse_macro_input};

#[proc_macro_attribute]
pub fn export(_args: TokenStream, input: TokenStream) -> TokenStream {
    let function = parse_macro_input!(input as ItemFn);
    match expand_export(&function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_export(function: &ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if !matches!(function.vis, syn::Visibility::Public(_)) {
        return Err(syn::Error::new_spanned(
            &function.vis,
            "eqts exports must be public",
        ));
    }
    if function.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            function.sig.asyncness,
            "eqts scalar exports do not support async functions yet",
        ));
    }
    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.generics,
            "eqts exports cannot be generic",
        ));
    }

    let name = &function.sig.ident;
    let wrapper = format_ident!("eqts_{name}");
    let mut parameters = Vec::new();
    let mut wrapper_inputs = Vec::new();
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
        let scalar = scalar_tokens(&argument.ty)?;
        let ident = &pattern.ident;
        let ty = &argument.ty;
        parameters.push(quote! {
            ::eqts::Parameter {
                name: stringify!(#ident),
                scalar: #scalar,
            }
        });
        wrapper_inputs.push(quote!(#ident: #ty));
        call_args.push(ident);
    }

    let (result, wrapper_result) = match &function.sig.output {
        ReturnType::Default => (quote!(::eqts::Scalar::Void), quote!()),
        ReturnType::Type(_, ty) => {
            let scalar = scalar_tokens(ty)?;
            (scalar, quote!(-> #ty))
        }
    };

    Ok(quote! {
        #function

        #[unsafe(no_mangle)]
        pub extern "C" fn #wrapper(#(#wrapper_inputs),*) #wrapper_result {
            #name(#(#call_args),*)
        }

        ::eqts::inventory::submit! {
            ::eqts::Function {
                name: stringify!(#name),
                parameters: &[#(#parameters),*],
                result: #result,
            }
        }
    })
}

fn scalar_tokens(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "eqts scalar exports require value types",
        ));
    };
    let Some(ident) = path.path.get_ident() else {
        return Err(syn::Error::new_spanned(
            ty,
            "eqts scalar exports require primitive types",
        ));
    };
    let variant = match ident.to_string().as_str() {
        "u8" => quote!(::eqts::Scalar::U8),
        "u16" => quote!(::eqts::Scalar::U16),
        "u32" => quote!(::eqts::Scalar::U32),
        "i8" => quote!(::eqts::Scalar::I8),
        "i16" => quote!(::eqts::Scalar::I16),
        "i32" => quote!(::eqts::Scalar::I32),
        "f32" => quote!(::eqts::Scalar::F32),
        "f64" => quote!(::eqts::Scalar::F64),
        _ => {
            return Err(syn::Error::new_spanned(ty, "unsupported eqts scalar type"));
        }
    };
    Ok(variant)
}
