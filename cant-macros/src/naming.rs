//! Identifier case helpers used to derive generated names.

use proc_macro2::Span;
use syn::Ident;

/// `movement` → `Movement`, `lifetime_tick` → `LifetimeTick`.
pub fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Builds `format_ident!` compatible input from a `snake_case` string.
pub fn system_struct_ident(fn_name: &Ident) -> Ident {
    let pascal = pascal_case(&fn_name.to_string());
    let mut s = String::with_capacity(pascal.len() + "System".len());
    s.push_str(&pascal);
    s.push_str("System");
    Ident::new(&s, fn_name.span())
}

/// Synthesizes an `Ident` with a specific span.
#[allow(dead_code)]
pub fn ident_at(name: &str, span: Span) -> Ident {
    Ident::new(name, span)
}
