//! `#[service]`, `#[repository]` and `#[datasource("name")]`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, LitStr};

use crate::{expand_autowire, field_initializers, snake_case, unique_ident};

/// Which layer a component belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComponentLayer {
    Service,
    Repository,
    Datasource,
}

impl ComponentLayer {
    fn kind_tokens(self) -> TokenStream2 {
        match self {
            ComponentLayer::Service => quote!(::stemson::core::registry::ComponentKind::Service),
            ComponentLayer::Repository => {
                quote!(::stemson::core::registry::ComponentKind::Repository)
            }
            ComponentLayer::Datasource => {
                quote!(::stemson::core::registry::ComponentKind::Datasource)
            }
        }
    }

    fn marker_tokens(self) -> TokenStream2 {
        match self {
            ComponentLayer::Service => quote!(::stemson::model::service::ServiceBase),
            ComponentLayer::Repository => quote!(::stemson::model::repository::RepositoryBase),
            ComponentLayer::Datasource => quote!(::stemson::model::datasource::DatasourceBase),
        }
    }

    fn label(self) -> &'static str {
        match self {
            ComponentLayer::Service => "service",
            ComponentLayer::Repository => "repository",
            ComponentLayer::Datasource => "datasource",
        }
    }
}

/// Common expansion used by the three component macros.
pub fn expand_component(input: TokenStream, layer: ComponentLayer) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input, layer, None) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `#[datasource("mysql")]`
pub fn expand_datasource(args: TokenStream, input: TokenStream) -> TokenStream {
    let name = parse_macro_input!(args as LitStr);
    let input = parse_macro_input!(input as DeriveInput);
    if name.value().trim().is_empty() {
        return syn::Error::new_spanned(name, "datasource name cannot be empty")
            .to_compile_error()
            .into();
    }
    match expand(&input, ComponentLayer::Datasource, Some(name.value())) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn strip_field_attributes(input: &DeriveInput) -> DeriveInput {
    let mut cleaned = input.clone();
    if let Data::Struct(data) = &mut cleaned.data {
        if let Fields::Named(fields) = &mut data.fields {
            for field in &mut fields.named {
                field
                    .attrs
                    .retain(|attr| !attr.path().is_ident("skip") && !attr.path().is_ident("value"));
            }
        }
    }
    cleaned
}

fn expand(
    input: &DeriveInput,
    layer: ComponentLayer,
    explicit_name: Option<String>,
) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let registry_name = explicit_name.unwrap_or_else(|| snake_case(&name.to_string()));
    let kind = layer.kind_tokens();
    let marker = layer.marker_tokens();
    let label = layer.label();

    let autowire = expand_autowire(input)?;
    // `#[skip]` / `#[value]` are consumed by the macro; they are not valid
    // attributes on the re-emitted struct definition.
    let cleaned_struct = strip_field_attributes(input);
    // Reuse the same field rules; `expand_autowire` already validated them.
    let _ = field_initializers(input)?;

    let build_fn = unique_ident("build", name);
    let build_doc = format!("Builds the {label} `{}` at startup.", name);

    Ok(quote! {
        #cleaned_struct

        #autowire

        impl #marker for #name {}

        #[doc = #build_doc]
        fn #build_fn(
            ctx: &::stemson::core::context::AppContext,
        ) -> ::anyhow::Result<::stemson::core::registry::ComponentValue> {
            let value = <#name as ::stemson::core::registry::Autowire>::autowire(ctx)?;
            ::std::result::Result::Ok(::std::sync::Arc::new(value))
        }

        ::stemson::inventory::submit! {
            ::stemson::core::registry::ComponentRegistration {
                name: #registry_name,
                kind: #kind,
                type_name: ::core::stringify!(#name),
                build: #build_fn,
                as_controller: ::core::option::Option::None,
            }
        }
    })
}
