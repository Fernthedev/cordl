use bitflags::bitflags;
use brocolib::runtime_metadata::TypeData;
use bytes::Bytes;
use itertools::Itertools;

use super::{cs_type_tag::CsTypeTag, writer::CppWritable};

use std::{hash::Hash, num, rc::Rc, sync::Arc};

#[derive(Debug, Eq, Hash, PartialEq, Clone, Default, PartialOrd, Ord)]
pub struct CsGenericTemplate {
    pub names: Vec<(CsGenericTemplateType, String)>,
}

#[derive(Debug, Eq, Hash, PartialEq, Clone, Default, PartialOrd, Ord)]
pub enum CsGenericTemplateType {
    #[default]
    Any,
    Reference,
}

impl CsGenericTemplate {
    pub fn make_typenames(names: impl Iterator<Item = String>) -> Self {
        CsGenericTemplate {
            names: names
                .into_iter()
                .map(|s| (CsGenericTemplateType::Any, s))
                .collect(),
        }
    }
    pub fn make_ref_types(names: impl Iterator<Item = String>) -> Self {
        CsGenericTemplate {
            names: names
                .into_iter()
                .map(|s| (CsGenericTemplateType::Reference, s))
                .collect(),
        }
    }

    pub fn just_names(&self) -> impl Iterator<Item = &String> {
        self.names.iter().map(|(_constraint, t)| t)
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, PartialOrd)]
pub struct CsCommentedString {
    pub data: String,
    pub comment: Option<String>,
}

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct CsUsingAlias {
    pub result: String,
    pub alias: String,
    pub template: Option<CsGenericTemplate>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CsMember {
    FieldDecl(CsField),
    MethodDecl(CsMethodDecl),
    Property(CsPropertyDecl),
    ConstructorDecl(CsConstructor),
    NestedUnion(CsNestedUnion),
    NestedStruct(CsNestedStruct),
    CppUsingAlias(CsUsingAlias),
    Comment(CsCommentedString),
    FieldLayout(CsFieldLayout),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CsNestedStruct {
    pub name: String,
    pub declarations: Vec<Rc<CsMember>>,
    pub is_enum: bool,
    pub is_class: bool,
    pub brief_comment: Option<String>,
    pub packing: Option<u8>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CsMethodData {
    pub estimated_size: usize,
    pub addrs: u64,
}

#[derive(Clone, Debug)]
pub struct CsMethodSizeData {
    pub cpp_method_name: String,
    pub method_name: String,
    pub declaring_type_name: String,
    pub declaring_classof_call: String,
    pub ret_ty: String,
    pub instance: bool,
    pub params: Vec<CsParam>,
    pub method_data: CsMethodData,

    // this is so bad
    pub method_info_lines: Vec<String>,
    pub method_info_var: String,

    pub template: Option<CsGenericTemplate>,
    pub generic_literals: Option<Vec<String>>,

    pub interface_clazz_of: String,
    pub is_final: bool,
    pub slot: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum CsValue {
    String(String),
    Bool(bool),
    
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),

    F32(f32),
    F64(f64),
    
    Object(Bytes),
    ValueType(Bytes),
    Null,
}


#[derive(Clone, Debug, PartialEq)]
pub struct CsField {
    pub name: String,
    pub field_ty: TypeData,
    pub instance: bool,
    pub readonly: bool,
    // is C# const
    // could be assumed from value though
    pub const_expr: bool,

    pub offset: Option<u32>,
    pub value: Option<CsValue>,
    pub brief_comment: Option<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct CsPropertyDecl {
    pub name: String,
    pub prop_ty: TypeData,
    pub instance: bool,
    pub getter: Option<String>,
    pub setter: Option<String>,
    /// Whether this property is one that's indexable (accessor methods take an index argument)
    pub indexable: bool,
    pub brief_comment: Option<String>,
}

bitflags! {
    #[derive(Debug, Clone, Hash, PartialEq, PartialOrd, Eq, Ord)]
    pub struct CsParamFlags: u8 {
        const A = 1;
        const B = 1 << 1;
        const C = 0b0000_0100;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CsParam {
    pub name: String,
    pub il2cpp_ty: TypeData,
    // TODO: Use bitflags to indicate these attributes
    // May hold:
    // const
    // May hold one of:
    // *
    // &
    // &&
    pub modifiers: CsParamFlags,
    pub def_value: Option<CsValue>,
}

bitflags! {
    pub struct MethodModifiers: u32 {
        const STATIC = 0b00000001;
        const VIRTUAL = 0b00000010;
        const OPERATOR = 0b00000100;
    }
}

// TODO: Generics
#[derive(Clone, Debug, PartialEq)]
pub struct CsMethodDecl {
    pub name: String,
    pub return_type: TypeData,
    pub parameters: Vec<CsParam>,
    pub instance: bool,
    pub template: Option<CsGenericTemplate>,
    pub method_data: Option<CsMethodData>,
    pub brief: Option<String>,
}

// TODO: Generics
#[derive(Clone, Debug)]
pub struct CsConstructor {
    pub cpp_name: String,
    pub parameters: Vec<CsParam>,
    pub template: Option<CsGenericTemplate>,

    pub brief: Option<String>,
    pub body: Option<Vec<Arc<dyn CppWritable>>>,
}

impl PartialEq for CsConstructor {
    fn eq(&self, other: &Self) -> bool {
        self.cpp_name == other.cpp_name
            && self.parameters == other.parameters
            && self.template == other.template
            && self.brief == other.brief
            // can't guarantee equality
            && self.body.is_some() == other.body.is_some()
    }
}


#[derive(Clone, Debug, PartialEq)]
pub struct CsNestedUnion {
    pub declarations: Vec<Rc<CsMember>>,
    pub brief_comment: Option<String>,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CsFieldLayout {
    pub field: CsField,
    // make struct with size [padding, field] packed with 1
    pub padding: u32,
    // make struct with size [alignment, field_size] default packed
    pub alignment: u32,
}
