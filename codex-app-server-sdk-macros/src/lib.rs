use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

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
