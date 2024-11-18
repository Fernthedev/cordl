use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use brocolib::{
    global_metadata::{
        FieldIndex, Il2CppTypeDefinition, MethodIndex, ParameterIndex, TypeDefinitionIndex,
        TypeIndex,
    },
    runtime_metadata::{Il2CppType, Il2CppTypeEnum, TypeData},
};
use clap::builder::Str;
use color_eyre::eyre::Context;
use itertools::Itertools;
use log::warn;
use std::io::Write;

use crate::{
    data::name_components::NameComponents,
    generate::{
        cpp::cpp_members::CppStaticAssert,
        cs_type::CsType,
        cs_type_tag::CsTypeTag,
        metadata::{Metadata, TypeUsage},
        offsets::{self, SizeInfo},
        type_extensions::{
            ParameterDefinitionExtensions, TypeDefinitionExtensions, TypeDefinitionIndexExtensions,
            TypeExtentions,
        },
        writer::{CppWritable, CppWriter, Sortable},
    },
};

use super::{
    config::CppGenerationConfig,
    cpp_context_collection::CppContextCollection,
    cpp_members::{
        CppConstructorDecl, CppConstructorImpl, CppFieldDecl, CppForwardDeclare, CppInclude,
        CppLine, CppMember, CppMethodDecl, CppMethodImpl, CppNestedStruct, CppNonMember, CppParam,
        CppTemplate, CppUsingAlias,
    },
};

pub const CORDL_TYPE_MACRO: &str = "CORDL_TYPE";
pub const __CORDL_IS_VALUE_TYPE: &str = "__IL2CPP_IS_VALUE_TYPE";
pub const __CORDL_BACKING_ENUM_TYPE: &str = "__CORDL_BACKING_ENUM_TYPE";

pub const CORDL_REFERENCE_TYPE_CONSTRAINT: &str = "::il2cpp_utils::il2cpp_reference_type";
pub const CORDL_NUM_ENUM_TYPE_CONSTRAINT: &str = "::cordl_internals::is_or_is_backed_by";
pub const CORDL_METHOD_HELPER_NAMESPACE: &str = "::cordl_internals";

// negative
pub const VALUE_TYPE_SIZE_OFFSET: u32 = 0x10;

pub const VALUE_TYPE_WRAPPER_SIZE: &str = "__IL2CPP_VALUE_TYPE_SIZE";
pub const REFERENCE_TYPE_WRAPPER_SIZE: &str = "__IL2CPP_REFERENCE_TYPE_SIZE";
pub const REFERENCE_TYPE_FIELD_SIZE: &str = "__fields";
pub const REFERENCE_WRAPPER_INSTANCE_NAME: &str = "::bs_hook::Il2CppWrapperType::instance";

pub const VALUE_WRAPPER_TYPE: &str = "::bs_hook::ValueType";
pub const ENUM_WRAPPER_TYPE: &str = "::bs_hook::EnumType";
pub const INTERFACE_WRAPPER_TYPE: &str = "::cordl_internals::InterfaceW";
pub const IL2CPP_OBJECT_TYPE: &str = "Il2CppObject";
pub const CORDL_NO_INCLUDE_IMPL_DEFINE: &str = "CORDL_NO_IMPL_INCLUDE";
pub const CORDL_ACCESSOR_FIELD_PREFIX: &str = "___";

pub const ENUM_PTR_TYPE: &str = "::bs_hook::EnumPtr";
pub const VT_PTR_TYPE: &str = "::bs_hook::VTPtr";

const SIZEOF_IL2CPP_OBJECT: u32 = 0x10;

#[derive(Debug, Clone, Default)]
pub struct CppTypeRequirements {
    pub forward_declares: HashSet<(CppForwardDeclare, CppInclude)>,

    // Only value types or classes
    pub required_def_includes: HashSet<CppInclude>,
    pub required_impl_includes: HashSet<CppInclude>,

    // Lists both types we forward declare or include
    pub depending_types: HashSet<CsTypeTag>,
}

impl CppTypeRequirements {
    pub fn add_forward_declare(&mut self, cpp_data: (CppForwardDeclare, CppInclude)) {
        // self.depending_types.insert(cpp_type.self_tag);
        self.forward_declares.insert(cpp_data);
    }

    pub fn add_def_include(&mut self, cpp_type: Option<&CppType>, cpp_include: CppInclude) {
        if let Some(cpp_type) = cpp_type {
            self.depending_types.insert(cpp_type.self_tag);
        }
        self.required_def_includes.insert(cpp_include);
    }
    pub fn add_impl_include(&mut self, cpp_type: Option<&CppType>, cpp_include: CppInclude) {
        if let Some(cpp_type) = cpp_type {
            self.depending_types.insert(cpp_type.self_tag);
        }
        self.required_impl_includes.insert(cpp_include);
    }
    pub fn add_dependency(&mut self, cpp_type: &CppType) {
        self.depending_types.insert(cpp_type.self_tag);
    }
    pub fn add_dependency_tag(&mut self, tag: CsTypeTag) {
        self.depending_types.insert(tag);
    }

    pub fn need_wrapper(&mut self) {
        self.add_def_include(
            None,
            CppInclude::new_exact("beatsaber-hook/shared/utils/base-wrapper-type.hpp"),
        );
    }
    pub fn needs_int_include(&mut self) {
        self.add_def_include(None, CppInclude::new_system("cstdint"));
    }
    pub fn needs_byte_include(&mut self) {
        self.add_def_include(None, CppInclude::new_system("cstddef"));
    }
    pub fn needs_math_include(&mut self) {
        self.add_def_include(None, CppInclude::new_system("cmath"));
    }
    pub fn needs_stringw_include(&mut self) {
        self.add_def_include(
            None,
            CppInclude::new_exact("beatsaber-hook/shared/utils/typedefs-string.hpp"),
        );
    }
    pub fn needs_arrayw_include(&mut self) {
        self.add_def_include(
            None,
            CppInclude::new_exact("beatsaber-hook/shared/utils/typedefs-array.hpp"),
        );
    }

    pub fn needs_byref_include(&mut self) {
        self.add_def_include(
            None,
            CppInclude::new_exact("beatsaber-hook/shared/utils/byref.hpp"),
        );
    }

    pub fn needs_enum_include(&mut self) {
        self.add_def_include(
            None,
            CppInclude::new_exact("beatsaber-hook/shared/utils/enum-type.hpp"),
        );
    }

    pub fn needs_value_include(&mut self) {
        self.add_def_include(
            None,
            CppInclude::new_exact("beatsaber-hook/shared/utils/value-type.hpp"),
        );
    }
}

#[derive(Clone, Debug)]
pub struct CppType {
    pub declarations: Vec<Arc<CppMember>>,
    pub nonmember_declarations: Vec<Arc<CppNonMember>>,
    pub implementations: Vec<Arc<CppMember>>,
    pub nonmember_implementations: Vec<Arc<CppNonMember>>,

    pub parent: Option<String>,
    pub interfaces: Vec<String>,

    pub is_value_type: bool,
    pub is_enum_type: bool,
    pub is_reference_type: bool,

    pub requirements: CppTypeRequirements,
    pub self_tag: CsTypeTag,

    /// contains the array of generic Il2CppType indexes
    pub generic_instantiations_args_types: Option<Vec<usize>>, // GenericArg -> Instantiation Arg
    pub method_generic_instantiation_map: HashMap<MethodIndex, Vec<TypeIndex>>, // MethodIndex -> Generic Args

    pub cpp_template: Option<CppTemplate>,
    pub cs_name_components: NameComponents,
    pub cpp_name_components: NameComponents,
    pub tag: CsTypeTag,
    pub(crate) prefix_comments: Vec<String>,
    pub packing: Option<u32>,
    pub size_info: Option<SizeInfo>,
}

impl CppType {
    pub fn write_impl(&self, writer: &mut CppWriter) -> color_eyre::Result<()> {
        self.write_impl_internal(writer)
    }

    pub fn write_def(&self, writer: &mut CppWriter) -> color_eyre::Result<()> {
        self.write_def_internal(writer, Some(&self.cpp_namespace()))
    }

    pub fn write_impl_internal(&self, writer: &mut CppWriter) -> color_eyre::Result<()> {
        self.nonmember_implementations
            .iter()
            .try_for_each(|d| d.write(writer))?;

        // Write all declarations within the type here
        self.implementations
            .iter()
            .sorted_by(|a, b| a.sort_level().cmp(&b.sort_level()))
            .try_for_each(|d| d.write(writer))?;

        Ok(())
    }

    fn write_def_internal(
        &self,
        writer: &mut CppWriter,
        namespace: Option<&str>,
    ) -> color_eyre::Result<()> {
        self.prefix_comments
            .iter()
            .try_for_each(|pc| writeln!(writer, "// {pc}").context("Prefix comment"))?;

        let type_kind = match self.is_value_type {
            true => "struct",
            false => "class",
        };

        // Just forward declare
        if let Some(n) = &namespace {
            writeln!(writer, "namespace {n} {{")?;
            writer.indent();
        }

        // Write type definition
        if let Some(generic_args) = &self.cpp_template {
            writeln!(writer, "// cpp template")?;
            generic_args.write(writer)?;
        }
        writeln!(writer, "// Is value type: {}", self.is_value_type)?;

        let clazz_name = self.cpp_name_components.formatted_name(false);

        writeln!(
            writer,
            "// CS Name: {}",
            self.cs_name_components.combine_all()
        )?;

        if let Some(packing) = &self.packing {
            writeln!(writer, "#pragma pack(push, {packing})")?;
        }

        let inherits = self.get_inherits().collect_vec();
        match inherits.is_empty() {
            true => writeln!(writer, "{type_kind} {CORDL_TYPE_MACRO} {clazz_name} {{")?,
            false => writeln!(
                writer,
                "{type_kind} {CORDL_TYPE_MACRO} {clazz_name} : {} {{",
                inherits
                    .into_iter()
                    .map(|s| format!("public {s}"))
                    .join(", ")
            )?,
        }

        writer.indent();

        // add public access
        writeln!(writer, "public:")?;
        writeln!(writer, "// Declarations")?;
        // Write all declarations within the type here
        self.declarations
            .iter()
            .sorted_by(|a, b| a.as_ref().partial_cmp(b.as_ref()).unwrap())
            .sorted_by(|a, b| {
                // fields and unions need to be sorted by offset to work correctly

                let a_offset = match a.as_ref() {
                    CppMember::FieldDecl(f) => f.offset,
                    CppMember::NestedUnion(u) => Some(u.offset),
                    _ => None,
                };

                let b_offset = match b.as_ref() {
                    CppMember::FieldDecl(f) => f.offset,
                    CppMember::NestedUnion(u) => Some(u.offset),
                    _ => None,
                };

                a_offset.cmp(&b_offset)
            })
            // sort by sort level after fields have been ordered correctly
            .sorted_by(|a, b| a.sort_level().cmp(&b.sort_level()))
            .try_for_each(|d| -> color_eyre::Result<()> {
                d.write(writer)?;
                writeln!(writer)?;
                Ok(())
            })?;

        writeln!(
            writer,
            "static constexpr bool {__CORDL_IS_VALUE_TYPE} = {};",
            self.is_value_type
        )?;
        // Type complete
        writer.dedent();
        writeln!(writer, "}};")?;

        if self.packing.is_some() {
            writeln!(writer, "#pragma pack(pop)")?;
        }

        // NON MEMBER DECLARATIONS
        writeln!(writer, "// Non member Declarations")?;

        self.nonmember_declarations
            .iter()
            .try_for_each(|d| -> color_eyre::Result<()> {
                d.write(writer)?;
                writeln!(writer)?;
                Ok(())
            })?;

        // Namespace complete
        if let Some(n) = namespace {
            writer.dedent();
            writeln!(writer, "}} // namespace end def {n}")?;
        }

        // TODO: Write additional meta-info here, perhaps to ensure correct conversions?
        Ok(())
    }

    pub fn write_type_trait(&self, writer: &mut CppWriter) -> color_eyre::Result<()> {
        if self.cpp_template.is_some() {
            // generic
            // macros from bs hook
            let type_trait_macro = if self.is_enum_type || self.is_value_type {
                "MARK_GEN_VAL_T"
            } else {
                "MARK_GEN_REF_PTR_T"
            };

            writeln!(
                writer,
                "{type_trait_macro}({});",
                self.cpp_name_components
                    .clone()
                    .remove_generics()
                    .remove_pointer()
                    .combine_all()
            )?;
        } else {
            // non-generic
            // macros from bs hook
            let type_trait_macro = if self.is_enum_type || self.is_value_type {
                "MARK_VAL_T"
            } else {
                "MARK_REF_PTR_T"
            };

            writeln!(
                writer,
                "{type_trait_macro}({});",
                self.cpp_name_components.remove_pointer().combine_all()
            )?;
        }

        Ok(())
    }

    pub fn make_cpp_type(metadata: &Metadata<'_>, tag: CsTypeTag, ty: CsType) -> CppType {
        todo!()
    }

    pub fn fill_from_il2cpp(&self, metadata: &Metadata<'_>, ctx_collection: &CppContextCollection) {
        todo!()
    }

    fn parent_joined_cpp_name(metadata: &Metadata, tdi: TypeDefinitionIndex) -> String {
        let ty_def = &metadata.metadata.global_metadata.type_definitions[tdi];

        let name = ty_def.name(metadata.metadata);

        if ty_def.declaring_type_index != u32::MAX {
            let declaring_ty =
                metadata.metadata_registration.types[ty_def.declaring_type_index as usize];

            if let TypeData::TypeDefinitionIndex(declaring_tdi) = declaring_ty.data {
                return Self::parent_joined_cpp_name(metadata, declaring_tdi) + "/" + name;
            } else {
                return declaring_ty.full_name(metadata.metadata) + "/" + name;
            }
        }

        ty_def.full_name(metadata.metadata, true)
    }

    fn il2cpp_byref(&mut self, cpp_name: String, typ: &Il2CppType) -> String {
        let requirements = &mut self.requirements;
        // handle out T or
        // ref T when T is a value type

        // typ.valuetype -> false when T&
        // apparently even if `T` is a valuetype
        if typ.is_param_out() || (typ.byref && !typ.valuetype) {
            requirements.needs_byref_include();
            return format!("ByRef<{cpp_name}>");
        }

        if typ.is_param_in() {
            requirements.needs_byref_include();

            return format!("ByRefConst<{cpp_name}>");
        }

        cpp_name
    }

    // Basically decides to use the template param name (if applicable)
    // instead of the generic instantiation of the type
    // TODO: Make this less confusing
    fn il2cpp_mvar_use_param_name<'a>(
        &mut self,
        metadata: &'a Metadata,
        method_index: MethodIndex,
        // use a lambda to do this lazily
        cpp_name: impl FnOnce(&mut CppType) -> String,
        typ: &'a Il2CppType,
    ) -> String {
        let tys = self.method_generic_instantiation_map.remove(&method_index);

        // fast path for generic param name
        // otherwise cpp_name() will default to generic param anyways
        let ret = match typ.ty {
            Il2CppTypeEnum::Mvar => match typ.data {
                TypeData::GenericParameterIndex(index) => {
                    let generic_param =
                        &metadata.metadata.global_metadata.generic_parameters[index];

                    let owner = generic_param.owner(metadata.metadata);
                    assert!(owner.is_method != u32::MAX);

                    generic_param.name(metadata.metadata).to_string()
                }
                _ => todo!(),
            },
            _ => cpp_name(self),
        };

        if let Some(tys) = tys {
            self.method_generic_instantiation_map
                .insert(method_index, tys);
        }

        ret
    }

    fn cppify_name_il2cpp(
        &mut self,
        ctx_collection: &CppContextCollection,
        metadata: &Metadata,
        typ: &Il2CppType,
        include_depth: usize,
        typ_usage: TypeUsage,
    ) -> NameComponents {
        let mut requirements = self.requirements.clone();

        let res = self.cppify_name_il2cpp_recurse(
            &mut requirements,
            ctx_collection,
            metadata,
            typ,
            include_depth,
            self.generic_instantiations_args_types.as_ref(),
            typ_usage,
        );

        self.requirements = requirements;

        res
    }

    /// [declaring_generic_inst_types] the generic instantiation of the declaring type
    fn cppify_name_il2cpp_recurse(
        &self,
        requirements: &mut CppTypeRequirements,
        ctx_collection: &CppContextCollection,
        metadata: &Metadata,
        typ: &Il2CppType,
        include_depth: usize,
        declaring_generic_inst_types: Option<&Vec<usize>>,
        typ_usage: TypeUsage,
    ) -> NameComponents {
        let add_include = include_depth > 0;
        let next_include_depth = if add_include { include_depth - 1 } else { 0 };

        let typ_tag = typ.data;

        match typ.ty {
            Il2CppTypeEnum::I1
            | Il2CppTypeEnum::U1
            | Il2CppTypeEnum::I2
            | Il2CppTypeEnum::U2
            | Il2CppTypeEnum::I4
            | Il2CppTypeEnum::U4
            | Il2CppTypeEnum::I8
            | Il2CppTypeEnum::U8
            | Il2CppTypeEnum::I
            | Il2CppTypeEnum::U => {
                requirements.needs_int_include();
            }
            Il2CppTypeEnum::R4 | Il2CppTypeEnum::R8 => {
                requirements.needs_math_include();
            }
            _ => (),
        };

        let ret = match typ.ty {
            // Commented so types use System.Object
            // might revert

            // Il2CppTypeEnum::Object => {
            //     requirements.need_wrapper();
            //     OBJECT_WRAPPER_TYPE.to_string()
            // }
            Il2CppTypeEnum::Object
            | Il2CppTypeEnum::Valuetype
            | Il2CppTypeEnum::Class
            | Il2CppTypeEnum::Typedbyref
            // ptr types
            | Il2CppTypeEnum::I
            | Il2CppTypeEnum::U => {
                let typ_cpp_tag: CsTypeTag = typ_tag.into();

                // Self
                if typ_cpp_tag == self.self_tag {
                    return self.cpp_name_components.clone();
                }

                // blacklist if needed
                if let TypeData::TypeDefinitionIndex(tdi) = typ.data {
                    let td = &metadata.metadata.global_metadata.type_definitions[tdi];

                    // TODO: Do we need generic inst types here? Hopefully not!
                    let _size = offsets::get_sizeof_type(td, tdi, None, metadata);

                    if metadata.blacklisted_types.contains(&tdi) {
                        // classes should return Il2CppObject*
                        if typ.ty == Il2CppTypeEnum::Class {
                            return NameComponents {
                                name: IL2CPP_OBJECT_TYPE.to_string(),
                                is_pointer: true,
                                generics: None,
                                namespace: None,
                                declaring_types: None,
                            };
                        }
                        return wrapper_type_for_tdi(td).to_string().into();
                    }
                }

                if add_include {
                    requirements.add_dependency_tag(typ_cpp_tag);
                }

                let to_incl = ctx_collection.get_context(typ_cpp_tag).unwrap_or_else(|| {
                    let t = &typ_cpp_tag.get_tdi().get_type_definition(metadata.metadata);

                    panic!(
                        "no context for type {typ:?} {}",
                        t.full_name(metadata.metadata, true)
                    )
                });

                let other_context_ty = ctx_collection.get_context_root_tag(typ_cpp_tag);
                let own_context_ty = ctx_collection.get_context_root_tag(self.self_tag);

                let typedef_incl = CppInclude::new_context_typedef(to_incl);
                let typeimpl_incl = CppInclude::new_context_typeimpl(to_incl);
                let to_incl_cpp_ty = ctx_collection
                    .get_cpp_type(typ.data.into())
                    .unwrap_or_else(|| panic!("Unable to get type to include {:?}", typ.data));

                let own_context = other_context_ty == own_context_ty;

                // - Include it
                // Skip including the context if we're already in it
                if !own_context {
                    match add_include {
                        // add def include
                        true => {
                            requirements
                                .add_def_include(Some(to_incl_cpp_ty), typedef_incl.clone());
                            requirements
                                .add_impl_include(Some(to_incl_cpp_ty), typeimpl_incl.clone());
                        }
                        // TODO: Remove?
                        // ignore nested types
                        // false if to_incl_cpp_ty.nested => {
                        // TODO: What should we do here?
                        // error!("Can't forward declare nested type! Including!");
                        // requirements.add_include(Some(to_incl_cpp_ty), inc);
                        // }
                        // forward declare
                        false => {
                            requirements.add_forward_declare((
                                CppForwardDeclare::from_cpp_type(to_incl_cpp_ty),
                                typedef_incl,
                            ));
                        }
                    }
                }
to_incl_cpp_ty.cpp_name_components.clone()

                // match to_incl_cpp_ty.is_enum_type || to_incl_cpp_ty.is_value_type {
                //     true => ret,
                //     false => format!("{ret}*"),
                // }
            }

            // Single dimension array
            Il2CppTypeEnum::Szarray => {
                requirements.needs_arrayw_include();

                let generic = match typ.data {
                    TypeData::TypeIndex(e) => {
                        let ty = &metadata.metadata_registration.types[e];

                        self.cppify_name_il2cpp_recurse(
                            requirements,
                            ctx_collection,
                            metadata,
                            ty,
                            include_depth,
                            declaring_generic_inst_types,
                            typ_usage,
                        )
                    }

                    _ => panic!("Unknown type data for array {typ:?}!"),
                };

                let generic_formatted = generic.combine_all();

                NameComponents {
                    name: "ArrayW".into(),
                    namespace: Some("".into()),
                    generics: Some(vec![
                        generic_formatted.clone(),
                        format!("::Array<{generic_formatted}>*"),
                    ]),
                    is_pointer: false,
                    ..Default::default()
                }
            }
            // multi dimensional array
            Il2CppTypeEnum::Array => {
                // FIXME: when stack further implements the TypeData::ArrayType we can actually implement this fully to be a multidimensional array, whatever that might mean
                warn!("Multidimensional array was requested but this is not implemented, typ: {typ:?}, instead returning Il2CppObject!");
                NameComponents {
                    name: IL2CPP_OBJECT_TYPE.to_string(),
                    is_pointer: true,
                    generics: None,
                    namespace: None,
                    declaring_types: None,
                }
            }
            Il2CppTypeEnum::Mvar => match typ.data {
                TypeData::GenericParameterIndex(index) => {
                    let generic_param: &brocolib::global_metadata::Il2CppGenericParameter =
                        &metadata.metadata.global_metadata.generic_parameters[index];

                    let owner = generic_param.owner(metadata.metadata);
                    assert!(owner.is_method != u32::MAX);

                    let (_gen_param_idx, gen_param) = owner
                        .generic_parameters(metadata.metadata)
                        .iter()
                        .find_position(|&p| p.name_index == generic_param.name_index)
                        .unwrap();

                    let method_index = MethodIndex::new(owner.owner_index);
                    let _method = &metadata.metadata.global_metadata.methods[method_index];

                    let method_args_opt =
                        self.method_generic_instantiation_map.get(&method_index);

                    if method_args_opt.is_none() {
                        return gen_param.name(metadata.metadata).to_string().into();
                    }

                    let method_args = method_args_opt.unwrap();

                    let ty_idx = method_args[gen_param.num as usize];
                    let ty = metadata
                        .metadata_registration
                        .types
                        .get(ty_idx as usize)
                        .unwrap();

                    self.cppify_name_il2cpp_recurse(
                        requirements,
                        ctx_collection,
                        metadata,
                        ty,
                        include_depth,
                        declaring_generic_inst_types,
                        TypeUsage::GenericArg,
                    )
                }
                _ => todo!(),
            },
            Il2CppTypeEnum::Var => match typ.data {
                // Il2CppMetadataGenericParameterHandle
                TypeData::GenericParameterIndex(index) => {
                    let generic_param: &brocolib::global_metadata::Il2CppGenericParameter =
                        &metadata.metadata.global_metadata.generic_parameters[index];

                    let owner = generic_param.owner(metadata.metadata);
                    let (_gen_param_idx, _gen_param) = owner
                        .generic_parameters(metadata.metadata)
                        .iter()
                        .find_position(|&p| p.name_index == generic_param.name_index)
                        .unwrap();

                    let ty_idx_opt = self
                        .generic_instantiations_args_types
                        .as_ref()
                        .and_then(|args| args.get(generic_param.num as usize))
                        .cloned();

                    // if template arg is not found
                    if ty_idx_opt.is_none() {
                        let gen_name = generic_param.name(metadata.metadata);

                        // true if the type is intentionally a generic template type and not a specialization
                        let has_generic_template =
                            self.cpp_template.as_ref().is_some_and(|template| {
                                template.just_names().any(|name| name == gen_name)
                            });

                        return match has_generic_template {
                            true => gen_name.to_string().into(),
                            false => panic!("/* TODO: FIX THIS, THIS SHOULDN'T HAPPEN! NO GENERIC INST ARGS FOUND HERE */ {gen_name}"),
                        };
                    }

                    let ty_var = &metadata.metadata_registration.types[ty_idx_opt.unwrap()];

                    let generics = &self
                        .cpp_name_components
                        .generics
                        .as_ref()
                        .expect("Generic instantiation args not made yet!");

                    let resolved_var = generics
                        .get(generic_param.num as usize)
                        .expect("No generic parameter at index found!")
                        .clone();

                    let is_pointer = !ty_var.valuetype
                    // if resolved_var exists in generic template, it can't be a pointer!
                        && (self.cpp_template.is_none()
                            || !self
                                .cpp_template
                                .as_ref()
                                .is_some_and(|t| t.just_names().any(|s| s == &resolved_var)));

                    NameComponents {
                        is_pointer,
                        name: resolved_var,
                        ..Default::default()
                    }

                    // This is for calculating on the fly
                    // which is slower and won't work for the reference type lookup fix
                    // we do in make_generic_args

                    // let ty_idx = ty_idx_opt.unwrap();

                    // let ty = metadata
                    //     .metadata_registration
                    //     .types
                    //     .get(ty_idx as usize)
                    //     .unwrap();
                    // self.cppify_name_il2cpp(ctx_collection, metadata, ty, add_include)
                }
                _ => todo!(),
            },
            Il2CppTypeEnum::Genericinst => match typ.data {
                TypeData::GenericClassIndex(e) => {
                    let mr = &metadata.metadata_registration;
                    let generic_class = mr.generic_classes.get(e).unwrap();
                    let generic_inst = mr
                        .generic_insts
                        .get(generic_class.context.class_inst_idx.unwrap())
                        .unwrap();

                    let new_generic_inst_types = &generic_inst.types;

                    let generic_type_def = &mr.types[generic_class.type_index];
                    let TypeData::TypeDefinitionIndex(tdi) = generic_type_def.data else {
                        panic!()
                    };

                    if add_include {
                        let generic_tag = CsTypeTag::from_type_data(typ.data, metadata.metadata);

                        // depend on both tdi and generic instantiation
                        requirements.add_dependency_tag(tdi.into());
                        requirements.add_dependency_tag(generic_tag);
                    }

                    let generic_types_formatted = new_generic_inst_types
                        // let generic_types_formatted = new_generic_inst_types
                        .iter()
                        .map(|t| mr.types.get(*t).unwrap())
                        // if t is a Var, we use the generic inst provided by the caller
                        // TODO: This commented code breaks generic params where we intentionally use the template name
                        // .map(|inst_t| match inst_t.data {
                        //     TypeData::GenericParameterIndex(gen_param_idx) => {
                        //         let gen_param =
                        //             &metadata.metadata.global_metadata.generic_parameters
                        //                 [gen_param_idx];
                        //         declaring_generic_inst_types
                        //             .and_then(|declaring_generic_inst_types| {
                        //                 // TODO: Figure out why we this goes out of bounds
                        //                 declaring_generic_inst_types.get(gen_param.num as usize)
                        //             })
                        //             .map(|t| &mr.types[*t])
                        //             // fallback to T since generic typedefs can be called
                        //             .unwrap_or(inst_t)
                        //     }
                        //     _ => inst_t,
                        // })
                        .map(|gen_arg_t| {
                            let should_include = gen_arg_t.valuetype;
                            let gen_include_detch = match should_include {
                                true => next_include_depth,
                                false => 0,
                            };

                            self.cppify_name_il2cpp_recurse(
                                requirements,
                                ctx_collection,
                                metadata,
                                gen_arg_t,
                                gen_include_detch,
                                // use declaring generic inst since we're cppifying generic args
                                declaring_generic_inst_types,
                                TypeUsage::GenericArg,
                            )
                        })
                        .map(|n| n.combine_all())
                        .collect_vec();

                    let generic_type_def = &mr.types[generic_class.type_index];
                    let type_def_name_components = self.cppify_name_il2cpp_recurse(
                        requirements,
                        ctx_collection,
                        metadata,
                        generic_type_def,
                        include_depth,
                        Some(new_generic_inst_types),
                        typ_usage,
                    );

                    // add generics to type def
                    NameComponents {
                        generics: Some(generic_types_formatted),
                        ..type_def_name_components
                    }
                }

                _ => panic!("Unknown type data for generic inst {typ:?}!"),
            },
            Il2CppTypeEnum::I1 => "int8_t".to_string().into(),
            Il2CppTypeEnum::I2 => "int16_t".to_string().into(),
            Il2CppTypeEnum::I4 => "int32_t".to_string().into(),
            Il2CppTypeEnum::I8 => "int64_t".to_string().into(),
            Il2CppTypeEnum::U1 => "uint8_t".to_string().into(),
            Il2CppTypeEnum::U2 => "uint16_t".to_string().into(),
            Il2CppTypeEnum::U4 => "uint32_t".to_string().into(),
            Il2CppTypeEnum::U8 => "uint64_t".to_string().into(),

            // https://learn.microsoft.com/en-us/nimbusml/concepts/types
            // https://en.cppreference.com/w/cpp/types/floating-point
            Il2CppTypeEnum::R4 => "float_t".to_string().into(),
            Il2CppTypeEnum::R8 => "double_t".to_string().into(),

            Il2CppTypeEnum::Void => "void".to_string().into(),
            Il2CppTypeEnum::Boolean => "bool".to_string().into(),
            Il2CppTypeEnum::Char => "char16_t".to_string().into(),
            Il2CppTypeEnum::String => {
                requirements.needs_stringw_include();
                "::StringW".to_string().into()
            }
            Il2CppTypeEnum::Ptr => {
                let generic = match typ.data {
                    TypeData::TypeIndex(e) => {
                        let ty = &metadata.metadata_registration.types[e];
                        self.cppify_name_il2cpp_recurse(
                            requirements,
                            ctx_collection,
                            metadata,
                            ty,
                            include_depth,
                            declaring_generic_inst_types,
                            typ_usage,
                        )
                    }

                    _ => panic!("Unknown type data for array {typ:?}!"),
                };

                let generic_formatted = generic.combine_all();

                NameComponents {
                    namespace: Some("cordl_internals".into()),
                    generics: Some(vec![generic_formatted]),
                    name: "Ptr".into(),
                    ..Default::default()
                }
            }
            // Il2CppTypeEnum::Typedbyref => {
            //     // TODO: test this
            //     if add_include && let TypeData::TypeDefinitionIndex(tdi) = typ.data {
            //         cpp_type.requirements.add_dependency_tag(tdi.into());
            //     }

            //     "::System::TypedReference".to_string()
            //     // "::cordl_internals::TypedByref".to_string()
            // },
            // TODO: Void and the other primitives
            _ => format!("/* UNKNOWN TYPE! {typ:?} */").into(),
        };

        ret
    }

    pub fn classof_cpp_name(&self) -> String {
        format!(
            "::il2cpp_utils::il2cpp_type_check::il2cpp_no_arg_class<{}>::get",
            self.cpp_name_components.combine_all()
        )
    }

    fn type_name_byref_fixup(ty: &Il2CppType, name: &str) -> String {
        match ty.valuetype {
            true => name.to_string(),
            false => format!("{name}*"),
        }
    }

    fn add_interface_operators(
        &mut self,
        metadata: &Metadata<'_>,
        ctx_collection: &CppContextCollection,
        config: &CppGenerationConfig,
        tdi: TypeDefinitionIndex,
    ) {
        let t = &metadata.metadata.global_metadata.type_definitions[tdi];

        for &interface_index in t.interfaces(metadata.metadata) {
            let int_ty = &metadata.metadata_registration.types[interface_index as usize];

            // We have an interface, lets do something with it
            let interface_name_il2cpp =
                &self.cppify_name_il2cpp(ctx_collection, metadata, int_ty, 0, TypeUsage::TypeName);
            let interface_cpp_name = interface_name_il2cpp.remove_pointer().combine_all();
            let interface_cpp_pointer = interface_name_il2cpp.as_pointer().combine_all();

            let operator_method_decl = CppMethodDecl {
                body: Default::default(),
                brief: Some(format!("Convert operator to {interface_cpp_name:?}")),
                cpp_name: interface_cpp_pointer.clone(),
                return_type: "".to_string(),
                instance: true,
                is_const: false,
                is_constexpr: true,
                is_no_except: !t.is_value_type() && !t.is_enum_type(),
                is_implicit_operator: true,
                is_explicit_operator: false,

                is_virtual: false,
                is_inline: true,
                parameters: vec![],
                template: None,
                prefix_modifiers: vec![],
                suffix_modifiers: vec![],
            };
            let helper_method_decl = CppMethodDecl {
                brief: Some(format!("Convert to {interface_cpp_name:?}")),
                is_implicit_operator: false,
                return_type: interface_cpp_pointer.clone(),
                cpp_name: format!("i_{}", config.sanitize_to_cpp_name(&interface_cpp_name)),
                ..operator_method_decl.clone()
            };

            let method_impl_template = self
                .cpp_template
                .as_ref()
                .is_some_and(|c| !c.names.is_empty())
                .then(|| self.cpp_template.clone())
                .flatten();

            let convert_line = match t.is_value_type() || t.is_enum_type() {
                true => {
                    // box
                    "static_cast<void*>(::il2cpp_utils::Box(this))".to_string()
                }
                false => "static_cast<void*>(this)".to_string(),
            };

            let body: Vec<Arc<dyn CppWritable>> = vec![Arc::new(CppLine::make(format!(
                "return static_cast<{interface_cpp_pointer}>({convert_line});"
            )))];
            let declaring_cpp_full_name = self.cpp_name_components.remove_pointer().combine_all();
            let operator_method_impl = CppMethodImpl {
                body: body.clone(),
                declaring_cpp_full_name: declaring_cpp_full_name.clone(),
                template: method_impl_template.clone(),
                ..operator_method_decl.clone().into()
            };

            let helper_method_impl = CppMethodImpl {
                body: body.clone(),
                declaring_cpp_full_name,
                template: method_impl_template,
                ..helper_method_decl.clone().into()
            };

            // operator
            self.declarations
                .push(CppMember::MethodDecl(operator_method_decl).into());
            self.implementations
                .push(CppMember::MethodImpl(operator_method_impl).into());

            // helper method
            self.declarations
                .push(CppMember::MethodDecl(helper_method_decl).into());
            self.implementations
                .push(CppMember::MethodImpl(helper_method_impl).into());
        }
    }

    fn create_size_assert(&mut self) {
        // FIXME: make this work with templated types that either: have a full template (complete instantiation), or only require a pointer (size should be stable)
        // for now, skip templated types
        if self.cpp_template.is_some() {
            return;
        }

        if let Some(size) = self.size_info.as_ref().map(|s| s.instance_size) {
            let cpp_name = self.cpp_name_components.remove_pointer().combine_all();

            assert!(!cpp_name.trim().is_empty(), "CPP Name cannot be empty!");

            let assert = CppStaticAssert {
                condition: format!("::cordl_internals::size_check_v<{cpp_name}, 0x{size:x}>"),
                message: Some("Size mismatch!".to_string()),
            };

            self.nonmember_declarations
                .push(Arc::new(CppNonMember::CppStaticAssert(assert)));
        } else {
            todo!("Why does this type not have a valid size??? {self:?}");
        }
    }

    ///
    /// add missing size for type
    ///
    fn create_size_padding(&mut self, metadata: &Metadata, tdi: TypeDefinitionIndex) {
        let cpp_type = {
            let this = &mut *self;
            this
        };

        // // get type metadata size
        let Some(type_definition_sizes) = &metadata.metadata_registration.type_definition_sizes
        else {
            return;
        };

        let metadata_size = &type_definition_sizes.get(tdi.index() as usize);

        let Some(metadata_size) = metadata_size else {
            return;
        };

        // // ignore types that aren't sized
        if metadata_size.instance_size == 0 || metadata_size.instance_size == u32::MAX {
            return;
        }

        // // if the size matches what we calculated, we're fine
        // if metadata_size.instance_size == calculated_size {
        //     return;
        // }
        // let remaining_size = metadata_size.instance_size.abs_diff(calculated_size);

        let Some(size_info) = cpp_type.size_info.as_ref() else {
            return;
        };

        // for all types, the size il2cpp metadata says the type should be, for generics this is calculated though
        let metadata_size_instance = size_info.instance_size;

        // align the calculated size to the next multiple of natural_alignment, similiar to what happens when clang compiles our generated code
        // this comes down to adding our size, and removing any bits that make it more than the next multiple of alignment
        #[cfg(feature = "il2cpp_v29")]
        let aligned_calculated_size = match size_info.natural_alignment as u32 {
            0 => size_info.calculated_instance_size,
            alignment => (size_info.calculated_instance_size + alignment) & !(alignment - 1),
        };
        #[cfg(feature = "il2cpp_v31")]
        let aligned_calculated_size = size_info.calculated_instance_size;

        // return if calculated layout size == metadata size
        if aligned_calculated_size == metadata_size_instance {
            return;
        }

        let remaining_size = metadata_size_instance.abs_diff(size_info.calculated_instance_size);

        // pack the remaining size to fit the packing of the type
        let closest_packing = |size: u32| match size {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 4,
            4 => 4,
            _ => 8,
        };

        let packing = cpp_type
            .packing
            .unwrap_or_else(|| closest_packing(size_info.calculated_instance_size));
        let packed_remaining_size = match packing == 0 {
            true => remaining_size,
            false => remaining_size & !(packing as u32 - 1),
        };

        // if the packed remaining size ends up being 0, don't emit padding
        if packed_remaining_size == 0 {
            return;
        }

        cpp_type.declarations.push(
            CppMember::FieldDecl(CppFieldDecl {
                cpp_name: format!("_cordl_size_padding[0x{packed_remaining_size:x}]").to_string(),
                field_ty: "uint8_t".into(),
                offset: Some(size_info.instance_size),
                instance: true,
                readonly: false,
                const_expr: false,
                value: None,
                brief_comment: Some(format!(
                    "Size padding 0x{:x} - 0x{:x} = 0x{remaining_size:x}, packed as 0x{packed_remaining_size:x}",
                    metadata_size_instance, size_info.calculated_instance_size
                )),
                is_private: false,
            })
            .into(),
        );
    }

    fn create_ref_size(&mut self) {
        let cpp_type = self;
        if let Some(size) = cpp_type.size_info.as_ref().map(|s| s.instance_size) {
            cpp_type.declarations.push(
                CppMember::FieldDecl(CppFieldDecl {
                    cpp_name: REFERENCE_TYPE_WRAPPER_SIZE.to_string(),
                    field_ty: "auto".to_string(),
                    offset: None,
                    instance: false,
                    readonly: false,
                    const_expr: true,
                    value: Some(format!("0x{size:x}")),
                    brief_comment: Some("The size of the true reference type".to_string()),
                    is_private: false,
                })
                .into(),
            );

            // here we push an instance field like uint8_t __fields[total_size - base_size] to make sure ref types are the exact size they should be
            let inherits = cpp_type.get_inherits().collect_vec();
            let fixup_size = match inherits.first() {
                Some(base_type) => format!("0x{size:x} - sizeof({base_type})"),
                None => format!("0x{size:x}"),
            };

            cpp_type.declarations.push(
                CppMember::FieldDecl(CppFieldDecl {
                    cpp_name: format!("{REFERENCE_TYPE_FIELD_SIZE}[{fixup_size}]"),
                    field_ty: "uint8_t".to_string(),
                    offset: None,
                    instance: true,
                    readonly: false,
                    const_expr: false,
                    value: Some("".into()),
                    brief_comment: Some(
                        "The size this ref type adds onto its base type, may evaluate to 0"
                            .to_string(),
                    ),
                    is_private: false,
                })
                .into(),
            );
        } else {
            todo!("Why does this type not have a valid size??? {:?}", cpp_type);
        }
    }

    fn create_enum_backing_type_constant(
        &mut self,
        metadata: &Metadata,
        ctx_collection: &CppContextCollection,
        tdi: TypeDefinitionIndex,
    ) {
        let t = tdi.get_type_definition(metadata.metadata);

        let backing_field_idx = t.element_type_index as usize;
        let backing_field_ty = &metadata.metadata_registration.types[backing_field_idx];

        let enum_base = self
            .cppify_name_il2cpp(
                ctx_collection,
                metadata,
                backing_field_ty,
                0,
                TypeUsage::TypeName,
            )
            .remove_pointer()
            .combine_all();

        self.declarations.push(
            CppMember::CppUsingAlias(CppUsingAlias {
                alias: __CORDL_BACKING_ENUM_TYPE.to_string(),
                result: enum_base,
                template: None,
            })
            .into(),
        );
    }

    fn create_enum_wrapper(
        &mut self,
        metadata: &Metadata,
        ctx_collection: &CppContextCollection,
        tdi: TypeDefinitionIndex,
    ) {
        let t = tdi.get_type_definition(metadata.metadata);
        let unwrapped_name = format!("__{}_Unwrapped", self.cpp_name());
        let backing_field = metadata
            .metadata_registration
            .types
            .get(t.element_type_index as usize)
            .unwrap();

        let enum_base = self
            .cppify_name_il2cpp(
                ctx_collection,
                metadata,
                backing_field,
                0,
                TypeUsage::TypeName,
            )
            .remove_pointer()
            .combine_all();

        let enum_entries = t
            .fields(metadata.metadata)
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let field_index = FieldIndex::new(t.field_start.index() + i as u32);

                (field_index, field)
            })
            .filter_map(|(field_index, field)| {
                let f_type = metadata
                    .metadata_registration
                    .types
                    .get(field.type_index as usize)
                    .unwrap();

                f_type.is_static().then(|| {
                    // enums static fields are always the enum values
                    let f_name = field.name(metadata.metadata);
                    let value = Self::field_default_value(metadata, field_index)
                        .expect("Enum without value!");

                    // prepend enum name with __E_ to prevent accidentally creating enum values that are reserved for builtin macros
                    format!("__E_{f_name} = {value},")
                })
            })
            .map(|s| -> CppMember { CppMember::CppLine(s.into()) });

        let nested_struct = CppNestedStruct {
            base_type: Some(enum_base.clone()),
            declaring_name: unwrapped_name.clone(),
            is_class: false,
            is_enum: true,
            is_private: false,
            declarations: enum_entries.map(Rc::new).collect(),
            brief_comment: Some(format!("Nested struct {unwrapped_name}")),
            packing: None,
        };
        self.declarations
            .push(CppMember::NestedStruct(nested_struct).into());

        let operator_body = format!("return static_cast<{unwrapped_name}>(this->value__);");
        let unwrapped_operator_decl = CppMethodDecl {
            cpp_name: Default::default(),
            instance: true,
            return_type: unwrapped_name,

            brief: Some("Conversion into unwrapped enum value".to_string()),
            body: Some(vec![Arc::new(CppLine::make(operator_body))]),
            is_const: true,
            is_constexpr: true,
            is_virtual: false,
            is_explicit_operator: false,
            is_implicit_operator: true,
            is_no_except: true,
            parameters: vec![],
            prefix_modifiers: vec![],
            suffix_modifiers: vec![],
            template: None,
            is_inline: true,
        };
        // convert to proper backing type
        let backing_operator_body = format!("return static_cast<{enum_base}>(this->value__);");
        let backing_operator_decl = CppMethodDecl {
            brief: Some("Conversion into unwrapped enum value".to_string()),
            return_type: enum_base,
            body: Some(vec![Arc::new(CppLine::make(backing_operator_body))]),
            is_explicit_operator: true,
            ..unwrapped_operator_decl.clone()
        };

        self.declarations
            .push(CppMember::MethodDecl(unwrapped_operator_decl).into());
        self.declarations
            .push(CppMember::MethodDecl(backing_operator_decl).into());
    }

    fn type_default_value(
        metadata: &Metadata,
        cpp_type: Option<&CppType>,
        ty: &Il2CppType,
    ) -> String {
        let matched_ty: &Il2CppType = match ty.data {
            // get the generic inst
            TypeData::GenericClassIndex(inst_idx) => {
                let gen_class = &metadata
                    .metadata
                    .runtime_metadata
                    .metadata_registration
                    .generic_classes[inst_idx];

                &metadata.metadata_registration.types[gen_class.type_index]
            }
            // get the underlying type of the generic param
            TypeData::GenericParameterIndex(param) => match param.is_valid() {
                true => {
                    let gen_param = &metadata.metadata.global_metadata.generic_parameters[param];

                    cpp_type
                        .and_then(|cpp_type| {
                            cpp_type
                                .generic_instantiations_args_types
                                .as_ref()
                                .and_then(|gen_args| gen_args.get(gen_param.num as usize))
                                .map(|t| &metadata.metadata_registration.types[*t])
                        })
                        .unwrap_or(ty)
                }
                false => ty,
            },
            _ => ty,
        };

        match matched_ty.valuetype {
            true => "{}".to_string(),
            false => "nullptr".to_string(),
        }
    }

    fn field_default_value(metadata: &Metadata, field_index: FieldIndex) -> Option<String> {
        metadata
            .metadata
            .global_metadata
            .field_default_values
            .as_vec()
            .iter()
            .find(|f| f.field_index == field_index)
            .map(|def| {
                let ty: &Il2CppType = metadata
                    .metadata_registration
                    .types
                    .get(def.type_index as usize)
                    .unwrap();

                // get default value for given type
                if !def.data_index.is_valid() {
                    return Self::type_default_value(metadata, None, ty);
                }

                todo!()
                // Self::default_value_blob(metadata, ty, def.data_index.index() as usize, true, true)
            })
    }
    fn param_default_value(metadata: &Metadata, parameter_index: ParameterIndex) -> Option<String> {
        metadata
            .metadata
            .global_metadata
            .parameter_default_values
            .as_vec()
            .iter()
            .find(|p| p.parameter_index == parameter_index)
            .map(|def| {
                let mut ty = metadata
                    .metadata_registration
                    .types
                    .get(def.type_index as usize)
                    .unwrap();

                todo!();

                // ty = Self::unbox_nullable_valuetype(metadata, ty);

                // This occurs when the type is `null` or `default(T)` for value types
                if !def.data_index.is_valid() {
                    return Self::type_default_value(metadata, None, ty);
                }

                if let Il2CppTypeEnum::Valuetype = ty.ty {
                    match ty.data {
                        TypeData::TypeDefinitionIndex(tdi) => {
                            let type_def = &metadata.metadata.global_metadata.type_definitions[tdi];

                            // System.Nullable`1
                            if type_def.name(metadata.metadata) == "Nullable`1"
                                && type_def.namespace(metadata.metadata) == "System"
                            {
                                ty = metadata
                                    .metadata_registration
                                    .types
                                    .get(type_def.byval_type_index as usize)
                                    .unwrap();
                            }
                        }
                        _ => todo!(),
                    }
                }

                todo!()

                // Self::default_value_blob(metadata, ty, def.data_index.index() as usize, true, true)
            })
    }

    fn create_valuetype_field_wrapper(&mut self) {
        let cpp_type = {
            let this = &mut *self;
            this
        };
        if cpp_type.size_info.is_none() {
            todo!("Why does this type not have a valid size??? {:?}", cpp_type);
        }

        let size = cpp_type
            .size_info
            .as_ref()
            .map(|s| s.instance_size)
            .unwrap();

        cpp_type.requirements.needs_byte_include();
        cpp_type.declarations.push(
            CppMember::FieldDecl(CppFieldDecl {
                cpp_name: VALUE_TYPE_WRAPPER_SIZE.to_string(),
                field_ty: "auto".to_string(),
                offset: None,
                instance: false,
                readonly: false,
                const_expr: true,
                value: Some(format!("0x{size:x}")),
                brief_comment: Some("The size of the true value type".to_string()),
                is_private: false,
            })
            .into(),
        );
        

        // cpp_type.declarations.push(
        //     CppMember::ConstructorDecl(CppConstructorDecl {
        //         cpp_name: cpp_type.cpp_name().clone(),
        //         parameters: vec![CppParam {
        //             name: "instance".to_string(),
        //             ty: format!("std::array<std::byte, {VALUE_TYPE_WRAPPER_SIZE}>"),
        //             modifiers: Default::default(),
        //             def_value: None,
        //         }],
        //         template: None,
        //         is_constexpr: true,
        //         is_explicit: true,
        //         is_default: false,
        //         is_no_except: true,
        //         is_delete: false,
        //         is_protected: false,
        //         base_ctor: Some((
        //             cpp_type.inherit.first().unwrap().to_string(),
        //             "instance".to_string(),
        //         )),
        //         initialized_values: Default::default(),
        //         brief: Some(
        //             "Constructor that lets you initialize the internal array explicitly".into(),
        //         ),
        //         body: Some(vec![]),
        //     })
        //     .into(),
        // );
    }

    fn create_valuetype_constructor(
        &mut self,
        metadata: &Metadata,
        ctx_collection: &CppContextCollection,
        config: &CppGenerationConfig,
        tdi: TypeDefinitionIndex,
    ) {
        let t = &metadata.metadata.global_metadata.type_definitions[tdi];

        let instance_fields = t
            .fields(metadata.metadata)
            .iter()
            .filter_map(|field| {
                let f_type = metadata
                    .metadata_registration
                    .types
                    .get(field.type_index as usize)
                    .unwrap();

                // ignore statics or constants
                if f_type.is_static() || f_type.is_constant() {
                    return None;
                }

                let f_type_cpp_name = self
                    .cppify_name_il2cpp(ctx_collection, metadata, f_type, 0, TypeUsage::FieldName)
                    .combine_all();

                // Get the inner type of a Generic Inst
                // e.g ReadOnlySpan<char> -> ReadOnlySpan<T>
                let def_value = Self::type_default_value(metadata, Some(self), f_type);

                let f_cpp_name = config
                    .name_cpp_plus(field.name(metadata.metadata), &[self.cpp_name().as_str()]);

                Some(CppParam {
                    name: f_cpp_name,
                    ty: f_type_cpp_name,
                    modifiers: "".to_string(),
                    // no default value for first param
                    def_value: Some(def_value),
                })
            })
            .collect_vec();

        if instance_fields.is_empty() {
            return;
        }
        // Maps into the first parent -> ""
        // so then Parent()
        let base_ctor = self.parent.as_ref().map(|s| (s.clone(), "".to_string()));

        let body: Vec<Arc<dyn CppWritable>> = instance_fields
            .iter()
            .map(|p| {
                let name = &p.name;
                CppLine::make(format!("this->{name} = {name};"))
            })
            .map(Arc::new)
            // Why is this needed? _sigh_
            .map(|arc| -> Arc<dyn CppWritable> { arc })
            .collect_vec();

        let params_no_def = instance_fields
            .iter()
            .cloned()
            .map(|mut c| {
                c.def_value = None;
                c
            })
            .collect_vec();

        let constructor_decl = CppConstructorDecl {
            cpp_name: self.cpp_name().clone(),
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: false,
            is_no_except: true,
            is_delete: false,
            is_protected: false,

            base_ctor,
            initialized_values: HashMap::new(),
            // initialize values with params
            // initialized_values: instance_fields
            //     .iter()
            //     .map(|p| (p.name.to_string(), p.name.to_string()))
            //     .collect(),
            parameters: params_no_def,
            brief: None,
            body: None,
        };

        let method_impl_template = if self
            .cpp_template
            .as_ref()
            .is_some_and(|c| !c.names.is_empty())
        {
            self.cpp_template.clone()
        } else {
            None
        };

        let constructor_impl = CppConstructorImpl {
            body,
            template: method_impl_template,
            parameters: instance_fields,
            declaring_full_name: self.cpp_name_components.remove_pointer().combine_all(),
            ..constructor_decl.clone().into()
        };

        self.declarations
            .push(CppMember::ConstructorDecl(constructor_decl).into());
        self.implementations
            .push(CppMember::ConstructorImpl(constructor_impl).into());
    }

    fn create_valuetype_default_constructors(&mut self) {
        // create the various copy and move ctors and operators
        let cpp_name = self.cpp_name();
        let wrapper = format!("{VALUE_WRAPPER_TYPE}<{VALUE_TYPE_WRAPPER_SIZE}>::instance");

        let move_ctor = CppConstructorDecl {
            cpp_name: cpp_name.clone(),
            parameters: vec![CppParam {
                ty: cpp_name.clone(),
                name: "".to_string(),
                modifiers: "&&".to_string(),
                def_value: None,
            }],
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: true,
            is_no_except: false,
            is_delete: false,
            is_protected: false,
            base_ctor: None,
            initialized_values: Default::default(),
            brief: None,
            body: None,
        };

        let copy_ctor = CppConstructorDecl {
            cpp_name: cpp_name.clone(),
            parameters: vec![CppParam {
                ty: cpp_name.clone(),
                name: "".to_string(),
                modifiers: "const &".to_string(),
                def_value: None,
            }],
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: true,
            is_no_except: false,
            is_delete: false,
            is_protected: false,
            base_ctor: None,
            initialized_values: Default::default(),
            brief: None,
            body: None,
        };

        let move_operator_eq = CppMethodDecl {
            cpp_name: "operator=".to_string(),
            return_type: format!("{cpp_name}&"),
            parameters: vec![CppParam {
                ty: cpp_name.clone(),
                name: "o".to_string(),
                modifiers: "&&".to_string(),
                def_value: None,
            }],
            instance: true,
            template: None,
            suffix_modifiers: vec![],
            prefix_modifiers: vec![],
            is_virtual: false,
            is_constexpr: true,
            is_const: false,
            is_no_except: true,
            is_implicit_operator: false,
            is_explicit_operator: false,

            is_inline: false,
            brief: None,
            body: Some(vec![
                Arc::new(CppLine::make(format!(
                    "this->{wrapper} = std::move(o.{wrapper});"
                ))),
                Arc::new(CppLine::make("return *this;".to_string())),
            ]),
        };

        let copy_operator_eq = CppMethodDecl {
            cpp_name: "operator=".to_string(),
            return_type: format!("{cpp_name}&"),
            parameters: vec![CppParam {
                ty: cpp_name.clone(),
                name: "o".to_string(),
                modifiers: "const &".to_string(),
                def_value: None,
            }],
            instance: true,
            template: None,
            suffix_modifiers: vec![],
            prefix_modifiers: vec![],
            is_virtual: false,
            is_constexpr: true,
            is_const: false,
            is_no_except: true,
            is_implicit_operator: false,
            is_explicit_operator: false,

            is_inline: false,
            brief: None,
            body: Some(vec![
                Arc::new(CppLine::make(format!("this->{wrapper} = o.{wrapper};"))),
                Arc::new(CppLine::make("return *this;".to_string())),
            ]),
        };

        self
            .declarations
            .push(CppMember::ConstructorDecl(move_ctor).into());
        self
            .declarations
            .push(CppMember::ConstructorDecl(copy_ctor).into());
        self
            .declarations
            .push(CppMember::MethodDecl(move_operator_eq).into());
        self
            .declarations
            .push(CppMember::MethodDecl(copy_operator_eq).into());
    }

    fn create_ref_default_constructor(&mut self) {
        let cpp_name = self.cpp_name().clone();

        let cs_name = self.name().clone();

        // Skip if System.ValueType or System.Enum
        if self.namespace() == "System" && (cs_name == "ValueType" || cs_name == "Enum") {
            return;
        }

        let default_ctor = CppConstructorDecl {
            cpp_name: cpp_name.clone(),
            parameters: vec![],
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: true,
            is_no_except: true,
            is_delete: false,
            is_protected: true,

            base_ctor: None,
            initialized_values: HashMap::new(),
            brief: Some("Default ctor for custom type constructor invoke".to_string()),
            body: None,
        };
        let copy_ctor = CppConstructorDecl {
            cpp_name: cpp_name.clone(),
            parameters: vec![CppParam {
                name: "".to_string(),
                modifiers: " const&".to_string(),
                ty: cpp_name.clone(),
                def_value: None,
            }],
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: true,
            is_no_except: true,
            is_delete: false,
            is_protected: false,

            base_ctor: None,
            initialized_values: HashMap::new(),
            brief: None,
            body: None,
        };
        let move_ctor = CppConstructorDecl {
            cpp_name: cpp_name.clone(),
            parameters: vec![CppParam {
                name: "".to_string(),
                modifiers: "&&".to_string(),
                ty: cpp_name.clone(),
                def_value: None,
            }],
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: true,
            is_no_except: true,
            is_delete: false,
            is_protected: false,

            base_ctor: None,
            initialized_values: HashMap::new(),
            brief: None,
            body: None,
        };

        self
            .declarations
            .push(CppMember::ConstructorDecl(default_ctor).into());
        self
            .declarations
            .push(CppMember::ConstructorDecl(copy_ctor).into());
        self
            .declarations
            .push(CppMember::ConstructorDecl(move_ctor).into());

        // // Delegates and such are reference types with no inheritance
        // if cpp_type.inherit.is_empty() {
        //     return;
        // }

        // let base_type = cpp_type
        //     .inherit
        //     .get(0)
        //     .expect("No parent for reference type?");

        // cpp_type.declarations.push(
        //     CppMember::ConstructorDecl(CppConstructorDecl {
        //         cpp_name: cpp_name.clone(),
        //         parameters: vec![CppParam {
        //             name: "ptr".to_string(),
        //             modifiers: "".to_string(),
        //             ty: "void*".to_string(),
        //             def_value: None,
        //         }],
        //         template: None,
        //         is_constexpr: true,
        //         is_explicit: true,
        //         is_default: false,
        //         is_no_except: true,
        //         is_delete: false,
        //         is_protected: false,

        //         base_ctor: Some((base_type.clone(), "ptr".to_string())),
        //         initialized_values: HashMap::new(),
        //         brief: None,
        //         body: Some(vec![]),
        //     })
        //     .into(),
        // );
    }
    fn make_interface_constructors(&mut self) {
        let cpp_name = self.cpp_name().clone();

        let base_type = self
            .parent
            .as_ref()
            .expect("No parent for interface type?");

        self.declarations.push(
            CppMember::ConstructorDecl(CppConstructorDecl {
                cpp_name: cpp_name.clone(),
                parameters: vec![CppParam {
                    name: "ptr".to_string(),
                    modifiers: "".to_string(),
                    ty: "void*".to_string(),
                    def_value: None,
                }],
                template: None,
                is_constexpr: true,
                is_explicit: true,
                is_default: false,
                is_no_except: true,
                is_delete: false,
                is_protected: false,

                base_ctor: Some((base_type.clone(), "ptr".to_string())),
                initialized_values: HashMap::new(),
                brief: None,
                body: Some(vec![]),
            })
            .into(),
        );
    }
    fn create_ref_default_operators(&mut self) {
        let cpp_name = self.cpp_name();

        // Skip if System.ValueType or System.Enum
        if self.namespace() == "System"
            && (self.cpp_name() == "ValueType" || self.cpp_name() == "Enum")
        {
            return;
        }

        // Delegates and such are reference types with no inheritance
        if self.get_inherits().count() > 0 {
            return;
        }

        self.declarations.push(
            CppMember::CppLine(CppLine {
                line: format!(
                    "
  constexpr {cpp_name}& operator=(std::nullptr_t) noexcept {{
    this->{REFERENCE_WRAPPER_INSTANCE_NAME} = nullptr;
    return *this;
  }};

  constexpr {cpp_name}& operator=(void* o) noexcept {{
    this->{REFERENCE_WRAPPER_INSTANCE_NAME} = o;
    return *this;
  }};

  constexpr {cpp_name}& operator=({cpp_name}&& o) noexcept = default;
  constexpr {cpp_name}& operator=({cpp_name} const& o) noexcept = default;
                "
                ),
            })
            .into(),
        );
    }

    fn delete_move_ctor(&mut self) {
        let cpp_type = {
            let this = &mut *self;
            this
        };
        let t = &cpp_type.cpp_name_components.name;

        let move_ctor = CppConstructorDecl {
            cpp_name: t.clone(),
            parameters: vec![CppParam {
                def_value: None,
                modifiers: "&&".to_string(),
                name: "".to_string(),
                ty: t.clone(),
            }],
            template: None,
            is_constexpr: false,
            is_explicit: false,
            is_default: false,
            is_no_except: false,
            is_protected: false,
            is_delete: true,
            base_ctor: None,
            initialized_values: Default::default(),
            brief: Some("delete move ctor to prevent accidental deref moves".to_string()),
            body: None,
        };

        cpp_type
            .declarations
            .push(CppMember::ConstructorDecl(move_ctor).into());
    }

    fn delete_copy_ctor(&mut self) {
        let cpp_type = {
            let this = &mut *self;
            this
        };
        let t = &cpp_type.cpp_name_components.name;

        let move_ctor = CppConstructorDecl {
            cpp_name: t.clone(),
            parameters: vec![CppParam {
                def_value: None,
                modifiers: "const&".to_string(),
                name: "".to_string(),
                ty: t.clone(),
            }],
            template: None,
            is_constexpr: false,
            is_explicit: false,
            is_default: false,
            is_no_except: false,
            is_delete: true,
            is_protected: false,
            base_ctor: None,
            initialized_values: Default::default(),
            brief: Some("delete copy ctor to prevent accidental deref copies".to_string()),
            body: None,
        };

        cpp_type
            .declarations
            .push(CppMember::ConstructorDecl(move_ctor).into());
    }

    fn add_default_ctor(&mut self, protected: bool) {
        let cpp_type = {
            let this = &mut *self;
            this
        };
        let t = &cpp_type.cpp_name_components.name;

        let default_ctor_decl = CppConstructorDecl {
            cpp_name: t.clone(),
            parameters: vec![],
            template: None,
            is_constexpr: true,
            is_explicit: false,
            is_default: false,
            is_no_except: false,
            is_delete: false,
            is_protected: protected,
            base_ctor: None,
            initialized_values: Default::default(),
            brief: Some("default ctor".to_string()),
            body: None,
        };

        let default_ctor_impl = CppConstructorImpl {
            body: vec![],
            declaring_full_name: cpp_type.cpp_name_components.remove_pointer().combine_all(),
            template: cpp_type.cpp_template.clone(),
            ..default_ctor_decl.clone().into()
        };

        cpp_type
            .declarations
            .push(CppMember::ConstructorDecl(default_ctor_decl).into());

        cpp_type
            .implementations
            .push(CppMember::ConstructorImpl(default_ctor_impl).into());
    }

    fn add_type_index_member(&mut self) {
        let cpp_type = {
            let this = &mut *self;
            this
        };
        let tdi: TypeDefinitionIndex = cpp_type.self_tag.get_tdi();

        let il2cpp_metadata_type_index = CppFieldDecl {
            cpp_name: "__IL2CPP_TYPE_DEFINITION_INDEX".into(),
            field_ty: "uint32_t".into(),
            offset: None,
            instance: false,
            readonly: true,
            const_expr: true,
            value: Some(tdi.index().to_string()),
            brief_comment: Some("IL2CPP Metadata Type Index".into()),
            is_private: false,
        };

        cpp_type
            .declarations
            .push(CppMember::FieldDecl(il2cpp_metadata_type_index).into());
    }

    fn delete_default_ctor(&mut self) {
        let cpp_type = {
            let this = &mut *self;
            this
        };
        let t = &cpp_type.cpp_name_components.name;

        let default_ctor = CppConstructorDecl {
            cpp_name: t.clone(),
            parameters: vec![],
            template: None,
            is_constexpr: false,
            is_explicit: false,
            is_default: false,
            is_no_except: false,
            is_delete: true,
            is_protected: false,
            base_ctor: None,
            initialized_values: Default::default(),
            brief: Some(
                "delete default ctor to prevent accidental value type instantiations of ref types"
                    .to_string(),
            ),
            body: None,
        };

        cpp_type
            .declarations
            .push(CppMember::ConstructorDecl(default_ctor).into());
    }

    fn create_ref_constructor(
        &mut self,
        declaring_type: &Il2CppTypeDefinition,
        m_params: &[CppParam],
        template: &Option<CppTemplate>,
    ) {
        if declaring_type.is_value_type() || declaring_type.is_enum_type() {
            return;
        }

        let params_no_default = m_params
            .iter()
            .cloned()
            .map(|mut c| {
                c.def_value = None;
                c
            })
            .collect_vec();

        let ty_full_cpp_name = self.cpp_name_components.combine_all();

        let decl: CppMethodDecl = CppMethodDecl {
            cpp_name: "New_ctor".into(),
            return_type: ty_full_cpp_name.clone(),
            parameters: params_no_default,
            template: template.clone(),
            body: None, // TODO:
            brief: None,
            is_no_except: false,
            is_constexpr: false,
            instance: false,
            is_const: false,
            is_implicit_operator: false,
            is_explicit_operator: false,

            is_virtual: false,
            is_inline: true,
            prefix_modifiers: vec![],
            suffix_modifiers: vec![],
        };

        // To avoid trailing ({},)
        let base_ctor_params = CppParam::params_names(&decl.parameters).join(", ");

        let allocate_call = format!(
            "THROW_UNLESS(::il2cpp_utils::NewSpecific<{ty_full_cpp_name}>({base_ctor_params}))"
        );

        let declaring_template = if self
            .cpp_template
            .as_ref()
            .is_some_and(|t| !t.names.is_empty())
        {
            self.cpp_template.clone()
        } else {
            None
        };

        let cpp_constructor_impl = CppMethodImpl {
            body: vec![Arc::new(CppLine::make(format!("return {allocate_call};")))],

            declaring_cpp_full_name: self.cpp_name_components.remove_pointer().combine_all(),
            parameters: m_params.to_vec(),
            template: declaring_template,
            ..decl.clone().into()
        };

        self.implementations
            .push(CppMember::MethodImpl(cpp_constructor_impl).into());

        self.declarations.push(CppMember::MethodDecl(decl).into());
    }

    pub fn get_inherits(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.parent)
            .flatten()
            .chain(self.interfaces.iter())
    }

    pub(crate) fn cpp_namespace(&self) -> String {
        self.cpp_name_components
            .namespace
            .clone()
            .unwrap_or("GlobalNamespace".to_owned())
    }

    pub(crate) fn namespace(&self) -> String {
        self.cs_name_components
            .namespace
            .clone()
            .unwrap_or("GlobalNamespace".to_owned())
    }

    pub(crate) fn cpp_name(&self) -> &std::string::String {
        &self.cpp_name_components.name
    }

    fn name(&self) -> &String {
        &self.cpp_name_components.name
    }
}

fn wrapper_type_for_tdi(td: &Il2CppTypeDefinition) -> &str {
    if td.is_enum_type() {
        return ENUM_WRAPPER_TYPE;
    }

    if td.is_value_type() {
        return VALUE_WRAPPER_TYPE;
    }

    if td.is_interface() {
        return INTERFACE_WRAPPER_TYPE;
    }

    IL2CPP_OBJECT_TYPE
}

///
/// This makes generic args for types such as ValueTask<List<T>> work
/// by recursively checking if any generic arg is a reference or numeric type (for enums)
///
fn parse_generic_arg(
    t: &Il2CppType,
    gen_name: String,
    cpp_type: &mut CppType,
    ctx_collection: &CppContextCollection,
    metadata: &Metadata<'_>,
    template_args: &mut Vec<(String, String)>,
) -> NameComponents {
    // If reference type, we use a template and add a requirement
    if !t.valuetype {
        template_args.push((
            CORDL_REFERENCE_TYPE_CONSTRAINT.to_string(),
            gen_name.clone(),
        ));
        return gen_name.into();
    }

    /*
       mscorelib.xml
       <type fullname="System.SByteEnum" />
       <type fullname="System.Int16Enum" />
       <type fullname="System.Int32Enum" />
       <type fullname="System.Int64Enum" />

       <type fullname="System.ByteEnum" />
       <type fullname="System.UInt16Enum" />
       <type fullname="System.UInt32Enum" />
       <type fullname="System.UInt64Enum" />
    */
    let enum_system_type_discriminator = match t.data {
        TypeData::TypeDefinitionIndex(tdi) => {
            let td = &metadata.metadata.global_metadata.type_definitions[tdi];
            let namespace = td.namespace(metadata.metadata);
            let name = td.name(metadata.metadata);

            if namespace == "System" {
                match name {
                    "SByteEnum" => Some(Il2CppTypeEnum::I1),
                    "Int16Enum" => Some(Il2CppTypeEnum::I2),
                    "Int32Enum" => Some(Il2CppTypeEnum::I4),
                    "Int64Enum" => Some(Il2CppTypeEnum::I8),
                    "ByteEnum" => Some(Il2CppTypeEnum::U1),
                    "UInt16Enum" => Some(Il2CppTypeEnum::U2),
                    "UInt32Enum" => Some(Il2CppTypeEnum::U4),
                    "UInt64Enum" => Some(Il2CppTypeEnum::U8),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    };

    let inner_enum_type = enum_system_type_discriminator.map(|e| Il2CppType {
        attrs: u16::MAX,
        byref: false,
        data: TypeData::TypeIndex(usize::MAX),
        pinned: false,
        ty: e,
        valuetype: true,
    });

    // if int, int64 etc.
    // this allows for enums to be supported
    if let Some(inner_enum_type) = inner_enum_type {
        let inner_enum_type_cpp = cpp_type
            .cppify_name_il2cpp(
                ctx_collection,
                metadata,
                &inner_enum_type,
                0,
                TypeUsage::GenericArg,
            )
            .combine_all();

        template_args.push((
            format!("{CORDL_NUM_ENUM_TYPE_CONSTRAINT}<{inner_enum_type_cpp}>",),
            gen_name.clone(),
        ));

        return gen_name.into();
    }

    let inner_type =
        cpp_type.cppify_name_il2cpp(ctx_collection, metadata, t, 0, TypeUsage::TypeName);

    match t.data {
        TypeData::GenericClassIndex(gen_class_idx) => {
            let gen_class = &metadata.metadata_registration.generic_classes[gen_class_idx];
            let gen_class_ty = &metadata.metadata_registration.types[gen_class.type_index];
            let TypeData::TypeDefinitionIndex(gen_class_tdi) = gen_class_ty.data else {
                todo!()
            };
            let gen_class_td = &metadata.metadata.global_metadata.type_definitions[gen_class_tdi];

            let gen_container = gen_class_td.generic_container(metadata.metadata);

            let gen_class_inst = &metadata.metadata_registration.generic_insts
                [gen_class.context.class_inst_idx.unwrap()];

            // this relies on the fact TDIs do not include their generic params
            let non_generic_inner_type = cpp_type.cppify_name_il2cpp(
                ctx_collection,
                metadata,
                gen_class_ty,
                0,
                TypeUsage::GenericArg,
            );

            let inner_generic_params = gen_class_inst
                .types
                .iter()
                .enumerate()
                .map(|(param_idx, u)| {
                    let t = metadata.metadata_registration.types.get(*u).unwrap();
                    let gen_param = gen_container
                        .generic_parameters(metadata.metadata)
                        .iter()
                        .find(|p| p.num as usize == param_idx)
                        .expect("No generic param at this num");

                    (t, gen_param)
                })
                .map(|(t, gen_param)| {
                    let inner_gen_name = gen_param.name(metadata.metadata).to_owned();
                    let mangled_gen_name =
                        format!("{inner_gen_name}_cordlgen_{}", template_args.len());
                    parse_generic_arg(
                        t,
                        mangled_gen_name,
                        cpp_type,
                        ctx_collection,
                        metadata,
                        template_args,
                    )
                })
                .map(|n| n.combine_all())
                .collect_vec();

            NameComponents {
                generics: Some(inner_generic_params),
                ..non_generic_inner_type
            }
        }
        _ => inner_type,
    }
}
