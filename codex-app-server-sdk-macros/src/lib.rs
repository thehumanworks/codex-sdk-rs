use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::parse_quote;
use syn::punctuated::Punctuated;
use syn::{Attribute, DeriveInput, Item, Meta, Path, Token, parse_macro_input};

#[proc_macro_derive(OpenAiSerializable)]
pub fn derive_openai_serializable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let generics = input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    TokenStream::from(quote! {
        impl #impl_generics ::codex_app_server_sdk::OpenAiSerializable for #ident #ty_generics #where_clause {
            fn openai_output_schema() -> ::codex_app_server_sdk::__private_serde_json::Value {
                ::codex_app_server_sdk::openai_json_schema_for::<Self>()
            }
        }
    })
}

#[proc_macro_attribute]
pub fn openai_type(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "openai_type does not accept arguments",
        )
        .to_compile_error()
        .into();
    }

    let mut item = parse_macro_input!(item as Item);
    let attrs = match &mut item {
        Item::Struct(item) => &mut item.attrs,
        Item::Enum(item) => &mut item.attrs,
        _ => {
            return syn::Error::new_spanned(
                item,
                "openai_type can only be used on structs or enums",
            )
            .to_compile_error()
            .into();
        }
    };

    ensure_derives(attrs);
    ensure_helper_attr(
        attrs,
        "serde",
        parse_quote!(#[serde(crate = "::codex_app_server_sdk::serde")]),
    );
    ensure_helper_attr(
        attrs,
        "schemars",
        parse_quote!(#[schemars(crate = "::codex_app_server_sdk::schemars")]),
    );

    TokenStream::from(quote!(#item))
}

fn ensure_derives(attrs: &mut Vec<Attribute>) {
    let required: [(&str, Path); 4] = [
        (
            "Serialize",
            parse_quote!(::codex_app_server_sdk::serde::Serialize),
        ),
        (
            "Deserialize",
            parse_quote!(::codex_app_server_sdk::serde::Deserialize),
        ),
        (
            "JsonSchema",
            parse_quote!(::codex_app_server_sdk::schemars::JsonSchema),
        ),
        (
            "OpenAiSerializable",
            parse_quote!(::codex_app_server_sdk::OpenAiSerializable),
        ),
    ];

    let missing: Vec<Path> = required
        .into_iter()
        .filter_map(|(needle, path)| (!has_derive(attrs, needle)).then_some(path))
        .collect();

    if !missing.is_empty() {
        attrs.push(parse_quote!(#[derive(#(#missing),*)]));
    }
}

fn has_derive(attrs: &[Attribute], needle: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }

        match &attr.meta {
            Meta::List(list) => list
                .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
                .map(|paths| {
                    paths.iter().any(|path| {
                        path.segments
                            .last()
                            .map(|segment| segment.ident == needle)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false),
            _ => false,
        }
    })
}

fn ensure_helper_attr(attrs: &mut Vec<Attribute>, helper: &str, attr: Attribute) {
    let has_helper_crate = attrs.iter().any(|existing| {
        existing.path().is_ident(helper)
            && existing
                .meta
                .to_token_stream()
                .to_string()
                .contains("crate")
    });

    if !has_helper_crate {
        attrs.push(attr);
    }
}
