//! Provides a utility to streamline parsing and manipulating and reconstructing the tokenstream representation of the #[app(arg1="val1", ...)] attribute

use proc_macro2::{Punct, Spacing, TokenStream as TokenStream2};
use quote::{ToTokens, TokenStreamExt, format_ident, quote};
use std::collections::HashMap;
use syn::{
    Attribute, Expr, ExprLit, Ident, Lit, Meta, Path, parse::Parser, parse_quote, spanned::Spanned,
};

#[derive(Debug, Clone)]
pub struct RticAttr {
    /// Attribute name, e.g. `task` for `#[task(...)]`
    pub name: Ident,
    pub elements: HashMap<String, Expr>,
}

impl RticAttr {
    /// Parse a #[app(arg1="val1", ...)] or #[task(core=N, param=M...)] macro attribute
    pub fn parse_from_attr(attribute: &Attribute) -> syn::Result<Self> {
        Self::from_meta(&attribute.meta)
    }

    /// Parse a `Meta` (an attribute) into an [RticAttr].
    pub fn from_meta(meta: &Meta) -> syn::Result<Self> {
        let name = meta.path().get_ident().cloned().ok_or_else(|| {
            syn::Error::new(
                meta.path().span(),
                "expected a single-segment attribute name",
            )
        })?;
        match meta {
            Meta::Path(_) => Ok(Self {
                name,
                elements: HashMap::new(),
            }),
            Meta::List(list) => Self::parse_from_tokens(list.tokens.clone(), name),
            Meta::NameValue(nv) => Err(syn::Error::new(
                nv.span(),
                "expected attribute of the form `#[name(key = value, ...)]`",
            )),
        }
    }

    /// Parse the tokenstream representation of the arguments of an #[app(arg1="val1", ...)] or #[task(core=N, param=M...)] macro attribute
    pub fn parse_from_tokens(tokens: TokenStream2, name: Ident) -> syn::Result<Self> {
        let mut elements = HashMap::new();
        syn::meta::parser(|meta| {
            let value: syn::Expr = meta
                .value()
                // Try parsing the assignment operator. On failure, set value = ().
                .map(|v| v.parse())
                .unwrap_or_else(|_| Ok(parse_quote!(())))?;
            if let Some(ident) = meta.path.get_ident() {
                elements.insert(ident.to_string(), value);
            }
            Ok(())
        })
        .parse2(tokens)?;

        Ok(Self { name, elements })
    }

    // ---------------------------------------------------------------------
    // Typed accessors
    // ---------------------------------------------------------------------

    /// Remove and parse `key` as a single-segment identifier, keeping its original span.
    pub fn take_ident(&mut self, key: &str) -> syn::Result<Option<Ident>> {
        match self.elements.remove(key) {
            None => Ok(None),
            Some(Expr::Path(path))
                if path.qself.is_none()
                    && path.path.segments.len() == 1
                    && path.path.leading_colon.is_none() =>
            {
                Ok(Some(path.path.segments[0].ident.clone()))
            }
            Some(other) => Err(syn::Error::new(
                other.span(),
                format!("`{key}` must be an identifier"),
            )),
        }
    }

    /// Remove and parse `key` as a path, e.g. `device = my_pac`.
    pub fn take_path(&mut self, key: &str) -> syn::Result<Option<Path>> {
        match self.elements.remove(key) {
            None => Ok(None),
            Some(Expr::Path(path)) if path.qself.is_none() => Ok(Some(path.path)),
            Some(other) => Err(syn::Error::new(
                other.span(),
                format!("`{key}` must be a path"),
            )),
        }
    }

    /// Remove and parse `key` as an unsigned integer literal, e.g. `priority = 2`.
    pub fn take_u16(&mut self, key: &str) -> syn::Result<Option<u16>> {
        self.take_int(key)
    }

    /// Remove and parse `key` as an unsigned integer literal, e.g. `cores = 2`.
    pub fn take_u32(&mut self, key: &str) -> syn::Result<Option<u32>> {
        self.take_int(key)
    }

    fn take_int<T>(&mut self, key: &str) -> syn::Result<Option<T>>
    where
        T: std::str::FromStr,
        <T as std::str::FromStr>::Err: std::fmt::Display,
    {
        let lit = match self.elements.remove(key) {
            None => return Ok(None),
            Some(Expr::Lit(ExprLit {
                lit: Lit::Int(lit), ..
            })) => lit,
            Some(other) => {
                return Err(syn::Error::new(
                    other.span(),
                    format!("`{key}` must be an integer literal"),
                ));
            }
        };
        lit.base10_digits()
            .parse()
            .map_err(|e| {
                syn::Error::new(
                    lit.span(),
                    format!("`{key}` must be an integer literal: {e}"),
                )
            })
            .map(Some)
    }

    /// Remove and parse `key` as an array of identifiers, e.g. `shared = [a, b]`,
    /// keeping the original spans of the identifiers.
    pub fn take_ident_array(&mut self, key: &str) -> syn::Result<Option<Vec<Ident>>> {
        let array = match self.elements.remove(key) {
            None => return Ok(None),
            Some(Expr::Array(array)) => array,
            Some(other) => {
                return Err(syn::Error::new(
                    other.span(),
                    format!("`{key}` must be an array of identifiers, e.g. `{key} = [a, b]`"),
                ));
            }
        };
        array
            .elems
            .into_iter()
            .map(|elem| match elem {
                Expr::Path(path)
                    if path.qself.is_none()
                        && path.path.segments.len() == 1
                        && path.path.leading_colon.is_none() =>
                {
                    Ok(path.path.segments[0].ident.clone())
                }
                other => Err(syn::Error::new(
                    other.span(),
                    format!("expected identifier in `{key}` array"),
                )),
            })
            .collect::<syn::Result<Vec<_>>>()
            .map(Some)
    }

    /// Remove and return the raw expression of `key`, if present.
    pub fn take_expr(&mut self, key: &str) -> Option<Expr> {
        self.elements.remove(key)
    }

    /// Borrow the raw expression of `key`, if present.
    pub fn get_expr(&self, key: &str) -> Option<&Expr> {
        self.elements.get(key)
    }

    // ---------------------------------------------------------------------
    // Supported-argument checks
    // ---------------------------------------------------------------------

    /// Returns an error for the first argument that is not in `supported`.
    pub fn ensure_supported(&self, supported: &[&str]) -> syn::Result<()> {
        if let Some(unknown) = self
            .elements
            .keys()
            .find(|key| !supported.contains(&key.as_str()))
        {
            return Err(syn::Error::new(
                self.elements[unknown].span(),
                format!(
                    "unknown argument `{unknown}`; expected one of: {}",
                    supported.join(", ")
                ),
            ));
        }
        Ok(())
    }

    /// Returns token streams that trigger a rustc warning for every argument not in
    /// `supported`. Rust has no stable way for proc-macros to emit warnings, so this
    /// uses the `#[deprecated]` item trick.
    pub fn unsupported_warnings(&self, supported: &[&str]) -> Vec<TokenStream2> {
        let attr_name = &self.name;
        self.elements
            .keys()
            .filter(|key| !supported.contains(&key.as_str()))
            .map(|unknown| {
                let warn_fn = format_ident!("rticx_warn_unknown_{}_{}", attr_name, unknown);
                let msg = format!("unknown `{attr_name}` argument `{unknown}`",);
                quote! {
                    #[deprecated(note = #msg)]
                    fn #warn_fn() {}
                    const _: fn() = #warn_fn;
                }
            })
            .collect()
    }

    /// Reconstructs the bare `key = value, ...` argument token stream of this
    /// attribute (without the `#[name(...)]` wrapper).
    ///
    /// Useful for passes that consume some arguments and hand the remaining
    /// ones back to the next compilation pass.
    pub fn args_tokens(&self) -> TokenStream2 {
        let args = self.elements.iter().map(|(name, value)| {
            let name = format_ident!("{name}");
            quote!(#name = #value)
        });
        let mut args_token_stream = TokenStream2::new();
        args_token_stream.append_separated(args, Punct::new(',', Spacing::Alone));
        args_token_stream
    }
}

impl ToTokens for RticAttr {
    /// Reconstruct the tokenstream representation of #[app(arg1="val1", ...)] macro attribute from the internal state of [Self]
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let attr_name = &self.name;
        let args_token_stream = self.args_tokens();
        let attribute: Attribute = parse_quote!(#[#attr_name(#args_token_stream)]);
        tokens.append_all(attribute.to_token_stream())
    }
}
