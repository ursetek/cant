//! Expansion of `#[system]`.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    Attribute, Expr, FnArg, Ident, ItemFn, Pat, ReturnType, Token, Type,
    parse::{Parse, ParseStream},
};

use crate::naming::system_struct_ident;

// ---------------------------------------------------------------------------
// Normalized input
// ---------------------------------------------------------------------------

/// A single entry in `#[resources(...)]`.
struct ResourceDecl {
    name: Ident,
    ty: Type,
    mutable: bool,
}

/// A single entry in `#[locals(...)]`.
struct LocalDecl {
    name: Ident,
    ty: Type,
    /// `Some(expr)` when written as `name: Type = expr`.
    init: Option<Expr>,
}

/// The normalized view of both attribute forms.
struct SystemInput {
    docs: Vec<Attribute>,
    system_name: Ident,
    hint_ns: Option<u64>,
    resources: Vec<ResourceDecl>,
    locals: Vec<LocalDecl>,
    commands_binding: Option<Ident>,
    fn_item: ItemFn,
}

/// Parser for the combined `#[system(...)]` argument list.
struct CombinedArgs {
    hint_ns: Option<u64>,
    resources: Vec<ResourceDecl>,
    locals: Vec<LocalDecl>,
    commands: Option<Option<Ident>>,
}

impl Parse for CombinedArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut hint_ns = None;
        let mut resources = Vec::new();
        let mut locals = Vec::new();
        let mut commands: Option<Option<Ident>> = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let key_str = key.to_string();
            match key_str.as_str() {
                "hint" => {
                    if hint_ns.is_some() {
                        return Err(syn::Error::new(
                            key.span(),
                            "duplicate `hint`",
                        ));
                    }
                    let content;
                    syn::parenthesized!(content in input);
                    let lit: syn::LitInt = content.parse()?;
                    hint_ns = Some(lit.base10_parse()?);
                }
                "resources" => {
                    if !resources.is_empty() {
                        return Err(syn::Error::new(
                            key.span(),
                            "duplicate `resources`",
                        ));
                    }
                    let content;
                    syn::parenthesized!(content in input);
                    parse_resource_list(&content, &mut resources)?;
                }
                "locals" => {
                    if !locals.is_empty() {
                        return Err(syn::Error::new(
                            key.span(),
                            "duplicate `locals`",
                        ));
                    }
                    let content;
                    syn::parenthesized!(content in input);
                    parse_local_list(&content, &mut locals)?;
                }
                "commands" => {
                    if commands.is_some() {
                        return Err(syn::Error::new(
                            key.span(),
                            "duplicate `commands`",
                        ));
                    }
                    if input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in input);
                        if content.is_empty() {
                            commands = Some(None);
                        } else {
                            let n: Ident = content.parse()?;
                            commands = Some(Some(n));
                        }
                    } else {
                        commands = Some(None);
                    }
                }
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown system option; expected `hint`, `resources`, \
                         `locals`, or `commands`",
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(Self {
            hint_ns,
            resources,
            locals,
            commands,
        })
    }
}

fn parse_resource_list(
    input: ParseStream,
    out: &mut Vec<ResourceDecl>,
) -> syn::Result<()> {
    while !input.is_empty() {
        let mutable = input.peek(Token![mut]);
        if mutable {
            input.parse::<Token![mut]>()?;
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty: Type = input.parse()?;
        out.push(ResourceDecl { name, ty, mutable });
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        } else {
            break;
        }
    }
    Ok(())
}

fn parse_local_list(
    input: ParseStream,
    out: &mut Vec<LocalDecl>,
) -> syn::Result<()> {
    while !input.is_empty() {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty: Type = input.parse()?;
        let init = if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            Some(input.parse::<Expr>()?)
        } else {
            None
        };
        out.push(LocalDecl { name, ty, init });
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        } else {
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let fn_item: ItemFn = match syn::parse2(item) {
        Ok(x) => x,
        Err(e) => return e.to_compile_error(),
    };

    let combined = if attr.is_empty() {
        None
    } else {
        match syn::parse2::<CombinedArgs>(attr) {
            Ok(c) => Some(c),
            Err(e) => return e.to_compile_error(),
        }
    };

    let input = match normalize(combined, fn_item) {
        Ok(x) => x,
        Err(e) => return e.to_compile_error(),
    };

    match generate(input) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error(),
    }
}

/// Merges the two attribute forms into a single `SystemInput`.
fn normalize(
    combined: Option<CombinedArgs>,
    fn_item: ItemFn,
) -> syn::Result<SystemInput> {
    let mut hint_ns = None;
    let mut resources = Vec::new();
    let mut locals = Vec::new();
    let mut commands_binding = None;
    let mut docs = Vec::new();
    let mut other_attrs = Vec::new();

    // Walk every attribute attached to the function.
    for attr in &fn_item.attrs {
        if attr.path().is_ident("hint") {
            if hint_ns.is_some() {
                return Err(syn::Error::new_spanned(attr, "duplicate `hint`"));
            }
            let lit: syn::LitInt = attr.parse_args()?;
            hint_ns = Some(lit.base10_parse()?);
        } else if attr.path().is_ident("resources") {
            if !resources.is_empty() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "duplicate `resources`",
                ));
            }
            attr.parse_args_with(|input: ParseStream| {
                parse_resource_list(input, &mut resources)
            })?;
        } else if attr.path().is_ident("locals") {
            if !locals.is_empty() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "duplicate `locals`",
                ));
            }
            attr.parse_args_with(|input: ParseStream| {
                parse_local_list(input, &mut locals)
            })?;
        } else if attr.path().is_ident("commands") {
            if commands_binding.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "duplicate `commands`",
                ));
            }
            // `#[commands]` or `#[commands(name)]`.
            match &attr.meta {
                syn::Meta::Path(_) => commands_binding = Some(None),
                syn::Meta::List(list) => {
                    if list.tokens.is_empty() {
                        commands_binding = Some(None);
                    } else {
                        let n: Ident = syn::parse2(list.tokens.clone())?;
                        commands_binding = Some(Some(n));
                    }
                }
                syn::Meta::NameValue(_) => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "`commands` does not take a value",
                    ));
                }
            }
        } else if attr.path().is_ident("doc") {
            docs.push(attr.clone());
        } else {
            other_attrs.push(attr.clone());
        }
    }

    // Merge the combined form, if present. Mixing is an error.
    let combined_used = combined.is_some();
    if let Some(c) = combined {
        if hint_ns.is_some()
            || !resources.is_empty()
            || !locals.is_empty()
            || commands_binding.is_some()
        {
            return Err(syn::Error::new(
                Span::call_site(),
                "cannot mix `#[system(...)]` with separate `#[hint]` / \
                 `#[resources]` / `#[locals]` / `#[commands]` attributes",
            ));
        }
        hint_ns = c.hint_ns;
        resources = c.resources;
        locals = c.locals;
        commands_binding = c.commands;
    }

    // Reconstruct the function without our consumed attributes and with the
    // remaining foreign attributes preserved for the generated `run`.
    let mut fn_item = fn_item;
    fn_item.attrs = other_attrs;

    let system_name = fn_item.sig.ident.clone();

    // Bail on unsupported signatures early.
    if !matches!(fn_item.sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &fn_item.sig.output,
            "system functions must return `()`",
        ));
    }
    for arg in &fn_item.sig.inputs {
        if let FnArg::Receiver(r) = arg {
            return Err(syn::Error::new_spanned(
                r,
                "system functions cannot take `self`",
            ));
        }
    }

    // Force command binding presence distinction: None means "no commands".
    let commands_binding = commands_binding.map(|opt| {
        opt.unwrap_or_else(|| Ident::new("commands", Span::call_site()))
    });

    let _ = combined_used;

    Ok(SystemInput {
        docs,
        system_name,
        hint_ns,
        resources,
        locals,
        commands_binding,
        fn_item,
    })
}

// ---------------------------------------------------------------------------
// Parameter classification
// ---------------------------------------------------------------------------

enum ParamKind {
    Entities,
    ReadComponent(Type),
    WriteComponent(Type),
}

struct Param {
    name: Ident,
    kind: ParamKind,
}

fn classify_arg(name: Ident, ty: &Type) -> syn::Result<Param> {
    if let Type::Reference(r) = ty {
        let mutable = r.mutability.is_some();
        if let Type::Slice(s) = &*r.elem {
            let elem = &*s.elem;
            if is_entity_type(elem) {
                if mutable {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "`&mut [Entity]` is not allowed; use `&[Entity]`",
                    ));
                }
                return Ok(Param {
                    name,
                    kind: ParamKind::Entities,
                });
            }
            if mutable {
                return Ok(Param {
                    name,
                    kind: ParamKind::WriteComponent((*elem).clone()),
                });
            }
            return Ok(Param {
                name,
                kind: ParamKind::ReadComponent((*elem).clone()),
            });
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "system parameters must be `&[Entity]`, `&[T]`, or `&mut [T]`",
    ))
}

fn is_entity_type(ty: &Type) -> bool {
    if let Type::Path(p) = ty
        && let Some(last) = p.path.segments.last()
    {
        return last.ident == "Entity";
    }
    false
}

// ---------------------------------------------------------------------------
// Code generation
// ---------------------------------------------------------------------------

const RESERVED: &[&str] =
    &["self", "view", "world", "commands", "__world", "__commands"];

fn generate(input: SystemInput) -> syn::Result<TokenStream> {
    let SystemInput {
        docs,
        system_name,
        hint_ns,
        resources,
        locals,
        commands_binding,
        fn_item,
    } = input;

    let struct_ident = system_struct_ident(&system_name);
    let name_str = system_name.to_string();

    // ---- collect and classify function parameters ----
    let mut params = Vec::new();
    let mut seen_entities = false;
    for arg in &fn_item.sig.inputs {
        let FnArg::Typed(pt) = arg else { continue };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new_spanned(
                &pt.pat,
                "system parameters must be simple identifiers",
            ));
        };
        let p = classify_arg(pi.ident.clone(), &pt.ty)?;
        if matches!(p.kind, ParamKind::Entities) {
            if seen_entities {
                return Err(syn::Error::new_spanned(
                    &pi.ident,
                    "`&[Entity]` may appear at most once",
                ));
            }
            seen_entities = true;
        }
        params.push(p);
    }

    // ---- name uniqueness ----
    let mut seen_names: Vec<Ident> = Vec::new();
    let mut register_name = |id: &Ident,
                             span_src: &dyn quote::ToTokens|
     -> syn::Result<()> {
        if RESERVED.contains(&id.to_string().as_str()) {
            return Err(syn::Error::new_spanned(
                span_src,
                format!("`{id}` is reserved and cannot be used as a binding"),
            ));
        }
        if seen_names.iter().any(|x| x == id) {
            return Err(syn::Error::new_spanned(
                span_src,
                format!("binding `{id}` is declared more than once"),
            ));
        }
        seen_names.push(id.clone());
        Ok(())
    };

    for p in &params {
        register_name(&p.name, &p.name)?;
    }
    for r in &resources {
        register_name(&r.name, &r.name)?;
    }
    for l in &locals {
        register_name(&l.name, &l.name)?;
    }
    if let Some(c) = &commands_binding {
        register_name(c, c)?;
    }

    // ---- component params, with indices matching declaration order ----
    let components: Vec<&Param> = params
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                ParamKind::ReadComponent(_) | ParamKind::WriteComponent(_)
            )
        })
        .collect();

    // ---- QueryMask construction ----
    let mut mask_stmts = Vec::new();
    for p in &components {
        match &p.kind {
            ParamKind::ReadComponent(ty) => mask_stmts.push(quote! {
                __mask.read(
                    world.component_id::<#ty>().unwrap_or_else(|| {
                        panic!(concat!(
                            "component `", stringify!(#ty), "` is not registered",
                        ))
                    }),
                );
            }),
            ParamKind::WriteComponent(ty) => mask_stmts.push(quote! {
                __mask.write(
                    world.component_id::<#ty>().unwrap_or_else(|| {
                        panic!(concat!(
                            "component `", stringify!(#ty), "` is not registered",
                        ))
                    }),
                );
            }),
            _ => {}
        }
    }

    // ---- resource plumbing ----
    let mut res_lookups = Vec::new();
    let mut res_fields = Vec::new();
    let mut res_inits = Vec::new();
    let mut res_access = Vec::new();
    for r in &resources {
        let ResourceDecl { name, ty, mutable } = r;
        let field = quote::format_ident!("{}_id", name);
        res_lookups.push(quote! {
            let #field: ::cant::ResourceId = world
                .resource_id::<#ty>()
                .unwrap_or_else(|| {
                    panic!(concat!(
                        "resource `", stringify!(#ty), "` is not registered",
                    ))
                });
        });
        res_fields.push(quote! { #field: ::cant::ResourceId, });
        res_inits.push(quote! { #field, });
        let acc = if *mutable {
            quote!(Write)
        } else {
            quote!(Read)
        };
        res_access.push(quote! {
            access.add_resource(#field, ::cant::Access::#acc);
        });
    }

    // ---- local state fields ----
    let mut local_fields = Vec::new();
    let mut local_inits = Vec::new();
    for l in &locals {
        let LocalDecl { name, ty, init } = l;
        local_fields.push(quote! { #name: #ty, });
        let init_expr = match init {
            Some(e) => quote!(#e),
            None => quote!(::core::default::Default::default()),
        };
        local_inits.push(quote! { #name: #init_expr, });
    }

    // ---- hint and structural flag ----
    let hint_stmt = match hint_ns {
        Some(h) => quote! { access.set_cost_hint_ns(#h); },
        None => quote!(),
    };
    let structural = if commands_binding.is_some() {
        quote! { access.mark_structural_write(); }
    } else {
        quote!()
    };

    // ---- run() prologue: resources, locals, commands ----
    let mut prologue = Vec::new();
    for r in &resources {
        let ResourceDecl { name, ty, mutable } = r;
        let field = quote::format_ident!("{}_id", name);
        if *mutable {
            prologue.push(quote! {
                // SAFETY: the access list marks this resource as exclusive;
                // the scheduler guarantees no conflicting system runs here.
                let #name: &mut #ty = unsafe {
                    __world.resource_mut::<#ty>(self.#field)
                }.expect(concat!(
                    "resource `", stringify!(#ty), "` is not registered",
                ));
            });
        } else {
            prologue.push(quote! {
                // SAFETY: shared borrow; aliasing reads are sound.
                let #name: &#ty = unsafe {
                    __world.resource::<#ty>(self.#field)
                }.expect(concat!(
                    "resource `", stringify!(#ty), "` is not registered",
                ));
            });
        }
    }
    for l in &locals {
        let name = &l.name;
        let ty = &l.ty;
        prologue.push(quote! {
            let #name: &mut #ty = &mut self.#name;
        });
    }
    if let Some(c) = &commands_binding {
        prologue.push(quote! {
            let #c: &mut ::cant::Commands = __commands;
        });
    }

    // ---- run() inner: per-chunk bindings ----
    let mut inner = Vec::new();
    for p in &params {
        match &p.kind {
            ParamKind::Entities => {
                let name = &p.name;
                inner.push(quote! {
                    let #name: &[::cant::Entity] = view.entities();
                });
            }
            ParamKind::ReadComponent(ty) => {
                let name = &p.name;
                let idx =
                    components.iter().position(|x| x.name == *name).unwrap();
                inner.push(quote! {
                    let #name: &[#ty] = view.column::<#ty>(#idx).unwrap_or_else(|| {
                        panic!(concat!(
                            "component column `", stringify!(#ty),
                            "` is missing from the archetype",
                        ))
                    });
                });
            }
            ParamKind::WriteComponent(ty) => {
                let name = &p.name;
                let idx =
                    components.iter().position(|x| x.name == *name).unwrap();
                inner.push(quote! {
                    // SAFETY: the access list declares `Write`; the
                    // scheduler guarantees no other system touches this
                    // column concurrently.
                    let #name: &mut [#ty] = unsafe {
                        view.esc_column_mut::<#ty>(#idx)
                    }.unwrap_or_else(|| {
                        panic!(concat!(
                            "component column `", stringify!(#ty),
                            "` is missing from the archetype",
                        ))
                    });
                });
            }
        }
    }

    let user_body = &fn_item.block;

    let run_body = if components.is_empty() {
        quote! {
            #(#prologue)*
            #user_body
        }
    } else {
        quote! {
            #(#prologue)*
            // SAFETY: the access list declared above is exactly the
            // components and resources this body touches. The scheduler
            // guarantees no conflicting access runs concurrently.
            unsafe {
                self.query.for_each_cell(__world, |view| {
                    #(#inner)*
                    #user_body
                });
            }
        }
    };

    // ---- foreign attributes preserved on `run` ----
    let preserved_attrs = &fn_item.attrs;

    Ok(quote! {
        #(#docs)*
        #[allow(non_camel_case_types, missing_debug_implementations)]
        pub struct #struct_ident {
            query: ::cant::Query,
            access: ::cant::AccessList,
            #(#res_fields)*
            #(#local_fields)*
        }

        impl #struct_ident {
            /// Builds the system, resolving all declared types against `world`.
            ///
            /// # Panics
            ///
            /// Panics if any declared component or resource is not registered.
            #[must_use]
            pub fn new(world: &mut ::cant::World) -> Self {
                let mut __mask = ::cant::QueryMask::new();
                #(#mask_stmts)*
                let query = world.prepare(__mask).expect("query preparation");

                #(#res_lookups)*

                let mut access = ::cant::AccessList::from_query(&query);
                #(#res_access)*
                #structural
                #hint_stmt

                Self {
                    query,
                    access,
                    #(#res_inits)*
                    #(#local_inits)*
                }
            }
        }

        impl ::cant::System for #struct_ident {
            fn name(&self) -> &'static str { #name_str }
            fn access(&self) -> &::cant::AccessList { &self.access }

            #(#preserved_attrs)*
            fn run(
                &mut self,
                __world: ::cant::UnsafeWorldCell<'_>,
                __commands: &mut ::cant::Commands,
            ) {
                #run_body
            }
        }
    })
}
