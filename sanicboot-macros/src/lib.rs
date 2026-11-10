//! Procedural macros for the `sanicboot` framework.
//!
//! | macro | Python counterpart |
//! |-------|-------------------|
//! | `#[service]` | `@Service` |
//! | `#[repository]` | `@Repository` |
//! | `#[datasource("mysql")]` | `@Datasource("mysql")` |
//! | `#[controller]` / `#[controller("/path")]` | `@Controller` |
//! | `#[derive(Autowired)]` | `@Autowired` fields |
//! | `#[value("dotted.key")]` field | `@Value("dotted.key")` |
//! | `#[validate(...)]` method | `@NotEmpty` / `@Length` / `@Range` / `@Pattern` / `@EnumString` / `@Assert` |
//! | `#[protected]` method | `@protected` / `@inject_user` |
//! | `#[authority([...])]` method | `@authority([...])` |
//!
//! All generated code refers to the framework as `::sanicboot`, so a crate
//! using these macros only needs `sanicboot` (plus its re-exported `axum`) in
//! scope.

mod component;
mod controller;
mod validate;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Ident, Type};

/// Derive constructor injection from struct fields.
///
/// ```ignore
/// #[derive(Autowired)]
/// pub struct UserController {
///     user_service: Arc<UserService>,
///     #[value("server.http.port")]
///     port: u16,
///     #[skip]
///     cache: OnceLock<u8>,
/// }
/// ```
#[proc_macro_derive(Autowired, attributes(skip, value))]
pub fn derive_autowired(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_autowire(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Register a service singleton and implement constructor injection.
#[proc_macro_attribute]
pub fn service(_args: TokenStream, input: TokenStream) -> TokenStream {
    component::expand_component(input, component::ComponentLayer::Service)
}

/// Register a repository and implement constructor injection.
#[proc_macro_attribute]
pub fn repository(_args: TokenStream, input: TokenStream) -> TokenStream {
    component::expand_component(input, component::ComponentLayer::Repository)
}

/// Register a datasource under an explicit name.
///
/// ```ignore
/// #[datasource("mysql")]
/// pub struct MysqlDataSource { config: Arc<Config>, #[skip] pool: OnceLock<MySqlPool> }
/// ```
#[proc_macro_attribute]
pub fn datasource(args: TokenStream, input: TokenStream) -> TokenStream {
    component::expand_datasource(args, input)
}

/// Declare the routes of a controller.
///
/// Without an argument the path is derived from the module path and the struct
/// name, exactly like the Python `@Controller`:
///
/// ```text
/// src/controller/user/user_controller.rs + struct AddUser
///   => POST /user/user_controller/add_user
/// ```
///
/// With an argument the path is explicit, like `@Controller("/getall")`.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::expand_controller(args, input)
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// Generate the `Autowire` implementation for a struct with named fields.
fn expand_autowire(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let initializers = field_initializers(input)?;
    Ok(quote! {
        impl ::sanicboot::core::registry::Autowire for #name {
            fn autowire(
                ctx: &::sanicboot::core::context::AppContext,
            ) -> ::anyhow::Result<Self> {
                ::std::result::Result::Ok(Self {
                    #(#initializers),*
                })
            }
        }
    })
}

/// Build one initializer per named field.
fn field_initializers(input: &DeriveInput) -> syn::Result<Vec<TokenStream2>> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "`#[derive(Autowired)]`, `#[service]`, `#[repository]`, `#[datasource]` and \
             `#[controller]` require a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "injected fields require a struct with named fields",
        ));
    };

    let mut initializers = Vec::new();
    for field in &fields.named {
        let ident = field.ident.as_ref().expect("named field");
        let ty = &field.ty;
        let mut skip = false;
        let mut value_key: Option<String> = None;
        for attr in &field.attrs {
            if attr.path().is_ident("skip") {
                skip = true;
            } else if attr.path().is_ident("value") {
                value_key = Some(parse_value_key(attr)?);
            } else {
                return Err(syn::Error::new_spanned(
                    attr,
                    "unsupported field attribute; use `#[skip]` or `#[value(\"dotted.key\")]`",
                ));
            }
        }

        let init = if skip {
            quote!(#ident: ::core::default::Default::default())
        } else if let Some(key) = value_key {
            quote!(#ident: ctx.inject_value::<#ty>(#key)?)
        } else if is_config_type(ty) || arc_inner(ty).map(is_config_type).unwrap_or(false) {
            // `Arc<Config>` is injected from the application context.
            quote!(#ident: ctx.config().clone())
        } else if let Some(inner) = arc_inner(ty) {
            quote!(#ident: ctx.resolve_component::<#inner>()?)
        } else {
            return Err(syn::Error::new_spanned(
                ty,
                "cannot inject this field; use `Arc<SomeComponent>` for a component, \
                 `Arc<Config>` for the configuration, `#[value(\"key\")]` for a value, \
                 or `#[skip]` for runtime state",
            ));
        };
        initializers.push(init);
    }
    Ok(initializers)
}

fn parse_value_key(attr: &syn::Attribute) -> syn::Result<String> {
    let lit: syn::LitStr = attr.parse_args()?;
    Ok(lit.value())
}

/// `Arc<T>` → `T`
fn arc_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Arc" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

/// Whether the type is `Config` (any path ending with `Config`).
fn is_config_type(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .map(|segment| segment.ident == "Config")
        .unwrap_or(false)
}

/// `CamelCase` → `snake_case`, matching `sanicboot::utils::text::to_snake_case`.
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && index > 0 && chars[index - 1].is_lowercase() {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Unique identifier for generated items.
fn unique_ident(prefix: &str, name: &Ident) -> Ident {
    Ident::new(
        &format!("__sanicboot_{prefix}_{}", snake_case(&name.to_string())),
        name.span(),
    )
}
