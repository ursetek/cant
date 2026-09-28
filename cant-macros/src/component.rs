//! Expansion of `#[derive(Component)]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

pub fn derive(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse2(input) {
        Ok(x) => x,
        Err(e) => return e.to_compile_error(),
    };
    let name = &input.ident;
    let name_str = name.to_string();
    quote! {
        impl ::cant::Component for #name {
            const NAME: &'static str = #name_str;
        }
    }
}
