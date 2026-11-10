//! Parser and code generator for `#[validate(...)]`.
//!
//! Grammar:
//!
//! ```text
//! #[validate(
//!     json,                                        // optional scope, default `all`
//!     not_empty(["username", "password"]),
//!     length("password", 5, 20, "Length of password must be 5 to 20"),
//!     range("age", 18, 80, "Age must in range 18 to 80"),
//!     pattern("email", r"^.+@.+$", match, "Email format invalid"),
//!     enum_string("gender", ["male", "female"], "gender must male or female"),
//!     assert("number", is_odd, "The number is not a odd"),
//!     assert_file("log", small_file, "File size can not exceed 99M"),
//! )]
//! ```

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parenthesized, Expr, ExprArray, Ident, Token};

/// Parameter scope of a validation block.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Json,
    Form,
    Args,
    Files,
}

impl Scope {
    fn tokens(self) -> TokenStream2 {
        match self {
            Scope::All => quote!(::sanicboot::request::context::ParamScope::All),
            Scope::Json => quote!(::sanicboot::request::context::ParamScope::Json),
            Scope::Form => quote!(::sanicboot::request::context::ParamScope::Form),
            Scope::Args => quote!(::sanicboot::request::context::ParamScope::Args),
            Scope::Files => quote!(::sanicboot::request::context::ParamScope::Files),
        }
    }
}

/// Matching mode of `pattern`.
#[derive(Clone, Copy)]
pub enum PatternMode {
    /// Anchored at the start (Python `re.match`).
    Match,
    /// Anywhere (Python `re.search`).
    Search,
}

/// One validation rule.
pub enum Rule {
    NotEmpty {
        keys: Vec<Expr>,
    },
    Length {
        key: Expr,
        min: Expr,
        max: Expr,
        message: Expr,
    },
    Range {
        key: Expr,
        min: Expr,
        max: Expr,
        message: Expr,
    },
    Pattern {
        key: Expr,
        pattern: Expr,
        mode: PatternMode,
        message: Expr,
    },
    EnumString {
        key: Expr,
        values: Vec<Expr>,
        message: Expr,
    },
    Assert {
        key: Expr,
        predicate: Expr,
        message: Expr,
    },
    AssertFile {
        key: Expr,
        predicate: Expr,
        message: Expr,
    },
}

impl Rule {
    fn tokens(&self) -> TokenStream2 {
        match self {
            Rule::NotEmpty { keys } => quote! {
                v.not_empty(&[#(#keys),*]);
            },
            Rule::Length {
                key,
                min,
                max,
                message,
            } => quote! {
                v.length(#key, (#min) as usize, (#max) as usize, #message);
            },
            Rule::Range {
                key,
                min,
                max,
                message,
            } => quote! {
                v.range(#key, (#min) as f64, (#max) as f64, #message);
            },
            Rule::Pattern {
                key,
                pattern,
                mode,
                message,
            } => {
                let mode = match mode {
                    PatternMode::Match => {
                        quote!(::sanicboot::core::decorator::validate::PatternMode::Match)
                    }
                    PatternMode::Search => {
                        quote!(::sanicboot::core::decorator::validate::PatternMode::Search)
                    }
                };
                quote! {
                    v.pattern(#key, #pattern, #mode, #message);
                }
            }
            Rule::EnumString {
                key,
                values,
                message,
            } => quote! {
                v.enum_string(#key, &[#(#values),*], #message);
            },
            Rule::Assert {
                key,
                predicate,
                message,
            } => quote! {
                v.assert(#key, #predicate, #message);
            },
            Rule::AssertFile {
                key,
                predicate,
                message,
            } => quote! {
                v.assert_file(#key, #predicate, #message);
            },
        }
    }
}

/// A whole `#[validate(...)]` attribute.
pub struct ValidateSpec {
    /// Scope applied to every rule in this attribute.
    pub scope: Scope,
    /// Rules, in declaration order.
    pub rules: Vec<Rule>,
}

impl Parse for ValidateSpec {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut scope = None;
        let mut rules = Vec::new();

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if input.peek(syn::token::Paren) {
                let content;
                parenthesized!(content in input);
                rules.push(parse_rule(&ident, &content)?);
            } else if let Some(parsed) = parse_scope(&ident) {
                if scope.is_some() {
                    return Err(syn::Error::new_spanned(
                        &ident,
                        "only one parameter scope may be given",
                    ));
                }
                scope = Some(parsed);
            } else {
                return Err(syn::Error::new_spanned(
                    &ident,
                    "expected a parameter scope (`all`, `json`, `form`, `args`, `files`) or a \
                     validation rule (`not_empty`, `length`, `range`, `pattern`, `enum_string`, \
                     `assert`, `assert_file`)",
                ));
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else if !input.is_empty() {
                return Err(input.error("expected `,` between validation entries"));
            }
        }

        Ok(ValidateSpec {
            scope: scope.unwrap_or(Scope::All),
            rules,
        })
    }
}

fn parse_scope(ident: &Ident) -> Option<Scope> {
    match ident.to_string().as_str() {
        "all" => Some(Scope::All),
        "json" => Some(Scope::Json),
        "form" => Some(Scope::Form),
        "args" => Some(Scope::Args),
        "files" => Some(Scope::Files),
        _ => None,
    }
}

fn parse_rule(name: &Ident, content: ParseStream) -> syn::Result<Rule> {
    let rule = match name.to_string().as_str() {
        "not_empty" => {
            let array: ExprArray = content.parse()?;
            Rule::NotEmpty {
                keys: array.elems.into_iter().collect(),
            }
        }
        "length" => Rule::Length {
            key: next_expr(content)?,
            min: next_expr(content)?,
            max: next_expr(content)?,
            message: next_expr(content)?,
        },
        "range" => Rule::Range {
            key: next_expr(content)?,
            min: next_expr(content)?,
            max: next_expr(content)?,
            message: next_expr(content)?,
        },
        "pattern" => {
            let key = next_expr(content)?;
            let pattern = next_expr(content)?;
            let mode = if content.peek(Token![match]) {
                content.parse::<Token![match]>()?;
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
                PatternMode::Match
            } else {
                let ident: Ident = content.parse()?;
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
                match ident.to_string().as_str() {
                    "match" => PatternMode::Match,
                    "search" => PatternMode::Search,
                    other => {
                        return Err(syn::Error::new_spanned(
                            ident,
                            format!("unknown pattern mode `{other}`, expected `match` or `search`"),
                        ))
                    }
                }
            };
            Rule::Pattern {
                key,
                pattern,
                mode,
                message: next_expr(content)?,
            }
        }
        "enum_string" => {
            let key = next_expr(content)?;
            let array: ExprArray = content.parse()?;
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
            Rule::EnumString {
                key,
                values: array.elems.into_iter().collect(),
                message: next_expr(content)?,
            }
        }
        "assert" => Rule::Assert {
            key: next_expr(content)?,
            predicate: next_expr(content)?,
            message: next_expr(content)?,
        },
        "assert_file" => Rule::AssertFile {
            key: next_expr(content)?,
            predicate: next_expr(content)?,
            message: next_expr(content)?,
        },
        other => {
            return Err(syn::Error::new_spanned(
                name,
                format!("unknown validation rule `{other}`"),
            ))
        }
    };

    if !content.is_empty() {
        return Err(content.error("too many arguments for this validation rule"));
    }
    Ok(rule)
}

fn next_expr(input: ParseStream) -> syn::Result<Expr> {
    let expr: Expr = input.parse()?;
    if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
    }
    Ok(expr)
}

/// Generate the validation block.
pub fn expand(specs: &[ValidateSpec]) -> TokenStream2 {
    let blocks = specs.iter().map(|spec| {
        let scope = spec.scope.tokens();
        let rules = spec.rules.iter().map(Rule::tokens);
        quote! {
            {
                let mut v = __sanicboot_ctx.validator(#scope);
                #(#rules)*
                v.check()?;
            }
        }
    });
    quote!(#(#blocks)*)
}
