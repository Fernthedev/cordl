use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse_quote;

use super::rust_name_components::RustNameComponents;

#[derive(Clone, Debug, Default)]
pub enum Visibility {
    Public,
    PublicCrate,
    #[default]
    Private,
}

#[derive(Clone)]
pub struct RustNamedItem {
    pub name: String,
    pub visibility: Visibility,
    pub item: RustItem,
}

/// Represents a Rust item, such as a struct, union, enum, or named type.
/// For usage in fields of structs, unions, and enums.
#[derive(Clone)]
pub enum RustItem {
    Struct(RustStruct),
    Union(RustUnion),
    Enum(RustEnum),
    NamedType(String),
}

#[derive(Clone)]
pub struct RustStruct {
    pub fields: Vec<RustField>,
    pub packing: Option<u32>,
}

#[derive(Clone)]
pub struct RustUnion {
    pub fields: Vec<RustField>,
}

#[derive(Clone)]
pub struct RustField {
    pub name: String,
    pub field_type: syn::Type,
    pub visibility: Visibility,
    pub offset: u32,
}

#[derive(Clone)]
pub struct RustEnum {
    pub variants: Vec<RustVariant>,
}

#[derive(Clone)]
pub struct RustVariant {
    pub name: syn::Ident,
    pub fields: Vec<RustField>,
}

#[derive(Clone)]
pub struct RustFunction {
    pub name: syn::Ident,
    pub params: Vec<RustParam>,
    pub return_type: Option<syn::Type>,
    pub body: Option<syn::Expr>,

    pub is_self: bool,
    pub is_ref: bool,
    pub is_mut: bool,
    pub visibility: Visibility,
}

#[derive(Clone)]
pub struct RustParam {
    pub name: syn::Ident,
    pub param_type: syn::Type,
}

#[derive(Clone)]
pub struct RustTrait {
    pub name: String,
    pub methods: Vec<RustFunction>,
    pub visibility: Visibility,
}

#[derive(Clone)]
pub struct RustImpl {
    pub trait_name: Option<String>,
    pub type_name: String,

    pub generics: Vec<Generic>,
    pub lifetimes: Vec<Lifetime>,

    pub methods: Vec<RustFunction>,
}

type Generic = String;
type Lifetime = String;

impl RustFunction {
    pub fn to_token_stream(&self) -> TokenStream {
        let name: syn::Ident = format_ident!("{}", self.name);
        let self_param: Option<syn::FnArg> = match self.is_self {
            true if self.is_mut && self.is_ref => Some(parse_quote! { &mut self }),
            true if self.is_ref => Some(parse_quote! { &self }),
            true if self.is_mut => Some(parse_quote! { mut self }),
            true => Some(parse_quote! { self }),
            false => None,
        };

        let params = self.params.iter().map(|p| -> syn::FnArg {
            let name = format_ident!("{}", p.name);
            let param_type = &p.param_type;
            parse_quote! { #name: #param_type }
        });
        let return_type: syn::ReturnType = match &self.return_type {
            Some(t_ty) => {
                parse_quote! { -> #t_ty }
            }
            None => parse_quote! {},
        };

        let visibility = self.visibility.to_token_stream();
        let mut tokens = match self_param {
            Some(self_param) => {
                quote! {
                    #visibility fn #name(#self_param, #(#params),*) #return_type
                }
            }
            None => {
                quote! {
                    #visibility fn #name(#(#params),*) #return_type
                }
            }
        };

        if let Some(body) = &self.body {
            tokens = quote! {
                #tokens {
                    #body
                }
            };
        } else {
            tokens = quote! {
                #tokens;
            };
        }

        tokens
    }
}

impl Visibility {
    pub fn to_token_stream(&self) -> syn::Visibility {
        match self {
            Visibility::Public => parse_quote! { pub },
            Visibility::PublicCrate => parse_quote! { pub(crate) },
            Visibility::Private => parse_quote! {},
        }
    }
}

impl ToString for Visibility {
    fn to_string(&self) -> String {
        match self {
            Visibility::Public => "pub".to_string(),
            Visibility::PublicCrate => "pub(crate)".to_string(),
            Visibility::Private => "".to_string(),
        }
    }
}
