//! `#[controller]` / `#[controller("/path")]`.
//!
//! Reproduces `backend/core/component/controller.py`:
//!
//! * the base path is derived from the module path and the struct name unless
//!   an explicit path is given;
//! * HTTP verbs come from method attributes (`#[post]`, `#[get("/me")]`, …),
//!   replacing `HTTPMethodView`'s `post`/`get` method names. The optional
//!   argument mounts the method on a sub-path;
//! * `#[validate]`, `#[protected]` and `#[authority]` run before the body, in
//!   the same order as the Python decorator stack.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, Attribute, Expr, ExprArray, ImplItem, ImplItemFn, ItemImpl, LitStr, Type,
};

use crate::validate::{expand as expand_validation, ValidateSpec};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl Verb {
    fn routing_fn(self) -> TokenStream2 {
        match self {
            Verb::Get => quote!(get),
            Verb::Post => quote!(post),
            Verb::Put => quote!(put),
            Verb::Delete => quote!(delete),
            Verb::Patch => quote!(patch),
            Verb::Head => quote!(head),
            Verb::Options => quote!(options),
        }
    }

    fn http_method(self) -> TokenStream2 {
        match self {
            Verb::Get => quote!(::stemson::http::Method::GET),
            Verb::Post => quote!(::stemson::http::Method::POST),
            Verb::Put => quote!(::stemson::http::Method::PUT),
            Verb::Delete => quote!(::stemson::http::Method::DELETE),
            Verb::Patch => quote!(::stemson::http::Method::PATCH),
            Verb::Head => quote!(::stemson::http::Method::HEAD),
            Verb::Options => quote!(::stemson::http::Method::OPTIONS),
        }
    }
}

const VERBS: [(&str, Verb); 7] = [
    ("get", Verb::Get),
    ("post", Verb::Post),
    ("put", Verb::Put),
    ("delete", Verb::Delete),
    ("patch", Verb::Patch),
    ("head", Verb::Head),
    ("options", Verb::Options),
];

/// Parse `#[get]`, `#[post("/login")]`, …
fn verb_of(attr: &Attribute) -> syn::Result<Option<(Verb, Option<LitStr>)>> {
    for (name, verb) in VERBS {
        if attr.path().is_ident(name) {
            return match &attr.meta {
                syn::Meta::Path(_) => Ok(Some((verb, None))),
                syn::Meta::List(list) => {
                    let lit: LitStr = syn::parse2(list.tokens.clone())?;
                    Ok(Some((verb, Some(lit))))
                }
                syn::Meta::NameValue(value) => Err(syn::Error::new_spanned(
                    value,
                    "use `#[get]` or `#[get(\"/sub/path\")]`",
                )),
            };
        }
    }
    Ok(None)
}

fn is_framework_attr(attr: &Attribute) -> bool {
    VERBS.iter().any(|(name, _)| attr.path().is_ident(name))
        || attr.path().is_ident("validate")
        || attr.path().is_ident("protected")
        || attr.path().is_ident("authority")
}

struct Route {
    verb: Verb,
    suffix: Option<String>,
    handler: String,
    method: ImplItemFn,
    validation: Vec<ValidateSpec>,
    protected: bool,
    authority: Option<Vec<Expr>>,
}

/// Entry point.
pub fn expand_controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let explicit = if args.is_empty() {
        None
    } else {
        match syn::parse::<LitStr>(args) {
            Ok(lit) => Some(lit),
            Err(err) => return err.to_compile_error().into(),
        }
    };
    let item = parse_macro_input!(input as ItemImpl);
    match expand(&item, explicit) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand(item: &ItemImpl, explicit: Option<LitStr>) -> syn::Result<TokenStream2> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "controllers cannot be generic",
        ));
    }
    let self_ty = &item.self_ty;
    let Type::Path(type_path) = self_ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "`#[controller]` must be applied to an inherent impl block of a named struct",
        ));
    };
    let struct_ident = type_path
        .path
        .segments
        .last()
        .map(|segment| segment.ident.clone())
        .ok_or_else(|| syn::Error::new_spanned(self_ty, "cannot determine the controller name"))?;
    let registry_name = crate::snake_case(&struct_ident.to_string());

    // ---------------------------------------------------------------- parse
    let mut routes: Vec<Route> = Vec::new();
    let mut kept_items: Vec<ImplItem> = Vec::new();

    for sub in &item.items {
        let ImplItem::Fn(function) = sub else {
            kept_items.push(sub.clone());
            continue;
        };

        let mut verb: Option<(Verb, Option<LitStr>)> = None;
        let mut validation = Vec::new();
        let mut protected = false;
        let mut authority = None;

        for attr in &function.attrs {
            if let Some(found) = verb_of(attr)? {
                if verb.is_some() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "a method can only declare one HTTP verb",
                    ));
                }
                verb = Some(found);
            } else if attr.path().is_ident("validate") {
                let list = attr.meta.require_list()?;
                validation.push(syn::parse2::<ValidateSpec>(list.tokens.clone())?);
            } else if attr.path().is_ident("protected") {
                protected = true;
            } else if attr.path().is_ident("authority") {
                let list = attr.meta.require_list()?;
                let array: ExprArray = syn::parse2(list.tokens.clone())?;
                authority = Some(array.elems.into_iter().collect());
            } else if attr.path().is_ident("doc") {
                // documentation stays on the generated impl
            } else {
                return Err(syn::Error::new_spanned(
                    attr,
                    "unsupported attribute on a controller method; expected `#[get]`, \
                     `#[post]`, `#[put]`, `#[delete]`, `#[patch]`, `#[head]`, `#[options]`, \
                     `#[validate(...)]`, `#[protected]` or `#[authority([...])]`",
                ));
            }
        }

        let Some((verb, suffix)) = verb else {
            if protected || authority.is_some() || !validation.is_empty() {
                return Err(syn::Error::new_spanned(
                    function,
                    "`#[protected]`, `#[authority]` and `#[validate]` are only valid on \
                     methods that declare a verb",
                ));
            }
            kept_items.push(sub.clone());
            continue;
        };

        if function.sig.asyncness.is_none() {
            return Err(syn::Error::new_spanned(
                &function.sig,
                "controller methods must be `async`",
            ));
        }

        let mut cleaned = function.clone();
        cleaned.attrs.retain(|attr| !is_framework_attr(attr));
        kept_items.push(ImplItem::Fn(cleaned.clone()));

        routes.push(Route {
            verb,
            suffix: suffix.map(|lit| lit.value()),
            handler: format!("__stemson_route_{}_{}", registry_name, routes.len()),
            method: cleaned,
            validation,
            protected,
            authority,
        });
    }

    if routes.is_empty() {
        return Err(syn::Error::new_spanned(
            item,
            "a controller needs at least one method annotated with `#[get]`, `#[post]`, \
             `#[put]`, `#[delete]`, `#[patch]`, `#[head]` or `#[options]`",
        ));
    }

    // -------------------------------------------------------------- handlers
    let handlers = routes.iter().map(|route| {
        let handler = format_ident!("{}", route.handler);
        let method = &route.method.sig.ident;
        let validation = expand_validation(&route.validation);
        let protected = if route.protected {
            quote!(__stemson_ctx.require_auth()?;)
        } else {
            quote!()
        };
        let authority = match &route.authority {
            Some(identities) => quote! {
                __stemson_ctx.require_identity(&[#(#identities),*])?;
            },
            None => quote!(),
        };

        quote! {
            async fn #handler(
                ::stemson::axum::extract::State(__stemson_state): ::stemson::axum::extract::State<
                    ::stemson::core::state::AppState,
                >,
                __stemson_request: ::stemson::axum::extract::Request,
            ) -> ::stemson::axum::response::Response {
                let __stemson_result: ::stemson::exception::ApiResult<
                    ::stemson::model::response::ApiResponse,
                > = async move {
                    let __stemson_ctx =
                        ::stemson::request::context::RequestContext::from_request(
                            &__stemson_state,
                            __stemson_request,
                        )
                        .await?;
                    #protected
                    #authority
                    #validation
                    let __stemson_controller =
                        __stemson_state.ctx().resolve::<#self_ty>()?;
                    #self_ty::#method(&*__stemson_controller, __stemson_ctx).await
                }
                .await;

                match __stemson_result {
                    ::std::result::Result::Ok(value) => {
                        ::stemson::axum::response::IntoResponse::into_response(value)
                    }
                    ::std::result::Result::Err(error) => {
                        ::stemson::axum::response::IntoResponse::into_response(error)
                    }
                }
            }
        }
    });

    // ----------------------------------------------------------- route table
    // Group by sub-path so every distinct path is registered exactly once.
    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for (index, route) in routes.iter().enumerate() {
        match groups
            .iter_mut()
            .find(|(suffix, _)| *suffix == route.suffix)
        {
            Some((_, indices)) => indices.push(index),
            None => groups.push((route.suffix.clone(), vec![index])),
        }
    }

    let mut route_builders = Vec::new();
    for (suffix, indices) in &groups {
        let handler_idents: Vec<_> = indices
            .iter()
            .map(|index| format_ident!("{}", routes[*index].handler))
            .collect();
        let first_verb = routes[indices[0]].verb.routing_fn();
        let first_handler = &handler_idents[0];
        let mut method_router = quote!(::stemson::axum::routing::#first_verb(#first_handler));
        for (route_index, handler) in indices.iter().zip(handler_idents.iter()).skip(1) {
            let verb = routes[*route_index].verb.routing_fn();
            method_router = quote!(#method_router.#verb(#handler));
        }

        let suffix_expr = match suffix {
            Some(suffix) => quote!(::core::option::Option::Some(#suffix)),
            None => quote!(::core::option::Option::None),
        };
        route_builders.push(quote! {
            {
                let route_path = ::stemson::core::route::join_path(&base_path, #suffix_expr);
                router = router.route(&route_path, #method_router);
            }
        });
    }

    let entries = routes.iter().map(|route| {
        let verb = route.verb.http_method();
        let suffix_expr = match &route.suffix {
            Some(suffix) => quote!(::core::option::Option::Some(#suffix)),
            None => quote!(::core::option::Option::None),
        };
        quote! {
            ::stemson::core::route::RouteEntry::new(
                ::stemson::core::route::join_path(&base_path, #suffix_expr),
                #verb,
                #registry_name,
            )
        }
    });

    let explicit_tokens = match &explicit {
        Some(lit) => quote!(::core::option::Option::Some(#lit)),
        None => quote!(::core::option::Option::None),
    };

    let build_fn = format_ident!("__stemson_build_{}", registry_name);
    let cast_fn = format_ident!("__stemson_as_controller_{}", registry_name);
    let doc = format!("Builds the controller `{struct_ident}` at startup.");

    let cleaned_impl = {
        let mut cleaned = item.clone();
        cleaned.items = kept_items;
        cleaned
    };

    Ok(quote! {
        #cleaned_impl

        #(#handlers)*

        impl ::stemson::model::controller::ControllerBase for #self_ty {
            fn name(&self) -> &'static str {
                #registry_name
            }

            fn base_path(&self) -> ::std::string::String {
                ::stemson::core::route::derive_path(
                    ::core::module_path!(),
                    ::core::env!("CARGO_CRATE_NAME"),
                    ::core::stringify!(#struct_ident),
                    #explicit_tokens,
                )
            }

            fn route_entries(
                &self,
            ) -> ::std::vec::Vec<::stemson::core::route::RouteEntry> {
                let base_path = self.base_path();
                ::std::vec![#(#entries),*]
            }

            fn routes(
                self: ::std::sync::Arc<Self>,
            ) -> ::stemson::axum::Router<::stemson::core::state::AppState> {
                let base_path = self.base_path();
                let mut router =
                    ::stemson::axum::Router::<::stemson::core::state::AppState>::new();
                #(#route_builders)*
                router
            }
        }

        #[doc = #doc]
        fn #build_fn(
            ctx: &::stemson::core::context::AppContext,
        ) -> ::anyhow::Result<::stemson::core::registry::ComponentValue> {
            let value =
                <#self_ty as ::stemson::core::registry::Autowire>::autowire(ctx)?;
            ::std::result::Result::Ok(::std::sync::Arc::new(value))
        }

        #[doc = "Downcasts the built controller to the router abstraction."]
        fn #cast_fn(
            value: ::stemson::core::registry::ComponentValue,
        ) -> ::core::option::Option<
            ::std::sync::Arc<dyn ::stemson::model::controller::ControllerBase>,
        > {
            let controller = value.downcast::<#self_ty>().ok()?;
            ::core::option::Option::Some(
                controller
                    as ::std::sync::Arc<dyn ::stemson::model::controller::ControllerBase>,
            )
        }

        ::stemson::inventory::submit! {
            ::stemson::core::registry::ComponentRegistration {
                name: #registry_name,
                kind: ::stemson::core::registry::ComponentKind::Controller,
                type_name: ::core::stringify!(#self_ty),
                build: #build_fn,
                as_controller: ::core::option::Option::Some(#cast_fn),
            }
        }
    })
}
