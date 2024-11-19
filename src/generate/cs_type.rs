use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    rc::Rc,
};

use byteorder::ReadBytesExt;

use brocolib::{
    global_metadata::{
        FieldIndex, Il2CppFieldDefinition, Il2CppTypeDefinition, MethodIndex, ParameterIndex,
        TypeDefinitionIndex, TypeIndex,
    },
    runtime_metadata::{Il2CppMethodSpec, Il2CppType, Il2CppTypeEnum, TypeData},
};
use itertools::Itertools;
use log::{debug, info, warn};

use crate::{
    data::name_components::NameComponents,
    generate::{
        cs_fields::{handle_static_fields, FieldInfo},
        cs_members::CsField,
        type_extensions::{
            Il2CppTypeEnumExtensions, ParameterDefinitionExtensions, TypeExtentions,
        },
    },
    helpers::cursor::ReadBytesExtensions,
    Endian,
};

use super::{
    cs_context_collection::TypeContextCollection,
    cs_fields::{handle_const_fields, handle_referencetype_fields, handle_valuetype_fields},
    cs_members::{
        CsGenericTemplate, CsMember, CsMethodData, CsMethodDecl, CsParam, CsParamFlags,
        CsPropertyDecl, CsValue,
    },
    cs_type_tag::CsTypeTag,
    metadata::Metadata,
    offsets::{self, SizeInfo},
    type_extensions::{MethodDefintionExtensions, TypeDefinitionExtensions},
};

#[derive(Debug, Clone, Default)]
pub struct CsTypeRequirements {
    // Lists both types we forward declare or include
    pub depending_types: HashSet<CsTypeTag>,
}

impl CsTypeRequirements {
    pub fn add_dependency(&mut self, ty: &CsType) {
        self.depending_types.insert(ty.self_tag);
    }
    pub fn add_dependency_tag(&mut self, tag: CsTypeTag) {
        self.depending_types.insert(tag);
    }
}

// Represents all of the information necessary for a C++ TYPE!
// A C# type will be TURNED INTO this
#[derive(Debug, Clone)]
pub struct CsType {
    pub self_tag: CsTypeTag,
    pub nested: bool,

    pub(crate) prefix_comments: Vec<String>,

    pub size_info: Option<SizeInfo>,
    pub packing: Option<u8>,

    // Computed by TypeDefinition.full_name()
    // Then fixed for generic types in CppContextCollection::make_generic_from/fill_generic_inst
    // pub cpp_name_components: NameComponents,
    pub cs_name_components: NameComponents,

    pub members: Vec<Rc<CsMember>>,

    pub is_value_type: bool,
    pub is_enum_type: bool,
    pub is_reference_type: bool,
    pub requirements: CsTypeRequirements,

    pub parent: Option<CsTypeTag>,
    pub interfaces: Vec<CsTypeTag>,
    pub generic_template: Option<CsGenericTemplate>, // Names of templates e.g T, TKey etc.

    /// contains the array of generic Il2CppType indexes
    ///
    /// for generic instantiation e.g Foo<T> -> Foo<int>
    pub generic_instantiations_args_types: Option<Vec<usize>>, // GenericArg idx -> Instantiation Arg
    pub method_generic_instantiation_map: HashMap<MethodIndex, Vec<TypeIndex>>, // MethodIndex -> Generic Args

    pub is_interface: bool,
    pub nested_types: HashSet<CsTypeTag>,
}

impl CsType {
    pub fn namespace(&self) -> String {
        self.cs_name_components
            .namespace
            .clone()
            .unwrap_or_default()
    }

    pub fn name(&self) -> &String {
        &self.cs_name_components.name
    }

    pub fn get_nested_types(&self) -> &HashSet<CsTypeTag> {
        &self.nested_types
    }

    pub fn get_tag_tdi(tag: TypeData) -> TypeDefinitionIndex {
        match tag {
            TypeData::TypeDefinitionIndex(tdi) => tdi,
            _ => panic!("Unsupported type: {tag:?}"),
        }
    }

    ////
    ///
    ///

    pub fn add_method_generic_inst(
        &mut self,
        method_spec: &Il2CppMethodSpec,
        metadata: &Metadata,
    ) -> &mut CsType {
        assert!(method_spec.method_inst_index != u32::MAX);

        let inst = metadata
            .metadata_registration
            .generic_insts
            .get(method_spec.method_inst_index as usize)
            .unwrap();

        self.method_generic_instantiation_map.insert(
            method_spec.method_definition_index,
            inst.types.iter().map(|t| *t as TypeIndex).collect(),
        );

        self
    }

    pub fn make_cs_type(
        metadata: &Metadata,
        tdi: TypeDefinitionIndex,
        tag: CsTypeTag,
        generic_inst_types: Option<&Vec<usize>>,
    ) -> Option<CsType> {
        // let iface = metadata.interfaces.get(t.interfaces_start);
        // Then, handle interfaces

        // Then, handle methods
        // - This includes constructors
        // inherited methods will be inherited

        let t = &metadata.metadata.global_metadata.type_definitions[tdi];

        // Generics
        // This is a generic type def
        // TODO: Constraints!
        let generics = t.generic_container_index.is_valid().then(|| {
            t.generic_container(metadata.metadata)
                .generic_parameters(metadata.metadata)
                .iter()
                .collect_vec()
        });

        let cpp_template = generics.as_ref().map(|g| {
            CsGenericTemplate::make_typenames(
                g.iter().map(|g| g.name(metadata.metadata).to_string()),
            )
        });

        let ns = t.namespace(metadata.metadata);
        let name = t.name(metadata.metadata);
        let full_name = t.full_name(metadata.metadata, false);

        if metadata.blacklisted_types.contains(&tdi) {
            info!("Skipping {full_name} ({tdi:?}) because it's blacklisted");

            return None;
        }

        // all nested types are unnested
        let nested = false; // t.declaring_type_index != u32::MAX;
        let cs_name_components = t.get_name_components(metadata.metadata);
        let is_pointer = cs_name_components.is_pointer;

        // TODO: Come up with a way to avoid this extra call to layout the entire type
        // We really just want to call it once for a given size and then move on
        // Every type should have a valid metadata size, even if it is 0
        let size_info: offsets::SizeInfo =
            offsets::get_size_info(t, tdi, generic_inst_types, metadata);

        // best results of cordl are when specified packing is strictly what is used, but experimentation may be required
        let packing = size_info.specified_packing;

        // Modified later for nested types
        let cpptype = CsType {
            self_tag: tag,
            nested,
            prefix_comments: vec![format!("Type: {ns}::{name}"), format!("{size_info:?}")],

            size_info: Some(size_info),
            packing,

            cs_name_components,

            members: Default::default(),

            is_value_type: t.is_value_type(),
            is_enum_type: t.is_enum_type(),
            is_reference_type: is_pointer,
            requirements: Default::default(),

            interfaces: Default::default(),
            parent: Default::default(),

            is_interface: t.is_interface(),
            generic_template: cpp_template,

            generic_instantiations_args_types: generic_inst_types.cloned(),
            method_generic_instantiation_map: Default::default(),

            nested_types: Default::default(),
        };

        // Nested type unnesting fix
        if t.declaring_type_index != u32::MAX {
            let declaring_ty = &metadata
                .metadata
                .runtime_metadata
                .metadata_registration
                .types[t.declaring_type_index as usize];

            let declaring_tag = CsTypeTag::from_type_data(declaring_ty.data, metadata.metadata);
            let declaring_tdi: TypeDefinitionIndex = declaring_tag.into();
            let _declaring_td = &metadata.metadata.global_metadata.type_definitions[declaring_tdi];
        }

        if t.parent_index == u32::MAX {
            if !t.is_interface() && t.full_name(metadata.metadata, true) != "System.Object" {
                info!("Skipping type: {ns}::{name} because it has parent index: {} and is not an interface!", t.parent_index);
                return None;
            }
        } else if metadata
            .metadata_registration
            .types
            .get(t.parent_index as usize)
            .is_none()
        {
            panic!("NO PARENT! But valid index found: {}", t.parent_index);
        }

        Some(cpptype)
    }

    pub fn fill_from_il2cpp(&mut self, metadata: &Metadata) {
        let tdi: TypeDefinitionIndex = self.self_tag.into();

        let _t = &metadata.metadata.global_metadata.type_definitions[tdi];

        self.make_parents(metadata, tdi);
        self.make_interfaces(metadata, tdi);

        self.make_nested_types(metadata, tdi);
        self.make_fields(metadata, tdi);
        self.make_properties(metadata, tdi);
        self.make_methods(metadata, tdi);

        if let Some(func) = metadata.custom_type_handler.get(&tdi) {
            func(self)
        }
    }

    fn make_parameters(
        &mut self,
        method: &brocolib::global_metadata::Il2CppMethodDefinition,
        metadata: &Metadata<'_>,
    ) -> Vec<CsParam> {
        method
            .parameters(metadata.metadata)
            .iter()
            .enumerate()
            .map(|(pi, param)| {
                let param_index = ParameterIndex::new(method.parameter_start.index() + pi as u32);

                self.make_parameter(param, param_index, metadata)
            })
            .collect()
    }

    fn make_parameter(
        &mut self,
        param: &brocolib::global_metadata::Il2CppParameterDefinition,
        param_index: ParameterIndex,
        metadata: &Metadata<'_>,
    ) -> CsParam {
        let param_type = metadata
            .metadata_registration
            .types
            .get(param.type_index as usize)
            .unwrap();

        let def_value = Self::param_default_value(metadata, param_index);

        CsParam {
            name: param.name(metadata.metadata).to_owned(),
            def_value,
            il2cpp_ty: param_type.data,
            modifiers: CsParamFlags::empty(),
        }
    }

    fn make_methods(&mut self, metadata: &Metadata, tdi: TypeDefinitionIndex) {
        let t = Self::get_type_definition(metadata, tdi);

        // Then, handle methods
        if t.method_count > 0 {
            // 2 because each method gets a method struct and method decl
            // a constructor will add an additional one for each
            self.members.reserve(2 * (t.method_count as usize + 1));

            // Then, for each method, write it out
            for (i, _method) in t.methods(metadata.metadata).iter().enumerate() {
                let method_index = MethodIndex::new(t.method_start.index() + i as u32);
                self.create_method(t, method_index, metadata, false);
            }
        }
    }

    fn make_fields(&mut self, metadata: &Metadata, tdi: TypeDefinitionIndex) {
        let t = Self::get_type_definition(metadata, tdi);

        // if no fields, skip
        if t.field_count == 0 {
            return;
        }

        let field_offsets = &metadata
            .metadata_registration
            .field_offsets
            .as_ref()
            .unwrap()[tdi.index() as usize];

        let mut offsets = Vec::<u32>::new();
        if let Some(sz) = offsets::get_size_of_type_table(metadata, tdi) {
            if sz.instance_size == 0 {
                // At this point we need to compute the offsets
                debug!(
                    "Computing offsets for TDI: {:?}, as it has a size of 0",
                    tdi
                );
                let _resulting_size = offsets::layout_fields(
                    metadata,
                    t,
                    tdi,
                    self.generic_instantiations_args_types.as_ref(),
                    Some(&mut offsets),
                    false,
                );
            }
        }
        let mut offset_iter = offsets.iter();

        fn get_offset<'a>(
            field: &Il2CppFieldDefinition,
            i: usize,
            mut iter: impl Iterator<Item = &'a u32>,
            field_offsets: &[u32],
            metadata: &Metadata<'_>,
            t: &Il2CppTypeDefinition,
        ) -> Option<u32> {
            let f_type = metadata
                .metadata_registration
                .types
                .get(field.type_index as usize)
                .unwrap();
            let f_name = field.name(metadata.metadata);

            match f_type.is_static() || f_type.is_constant() {
                // return u32::MAX for static fields as an "invalid offset" value
                true => None,
                false => Some({
                    // If we have a hotfix offset, use that instead
                    // We can safely assume this always returns None even if we "next" past the end
                    let offset = if let Some(computed_offset) = iter.next() {
                        *computed_offset
                    } else {
                        field_offsets[i]
                    };

                    if offset < metadata.object_size() as u32 {
                        warn!("Field {f_name} ({offset:x}) of {} is smaller than object size {:x} is value type {}",
                            t.full_name(metadata.metadata, true),
                            metadata.object_size(),
                            t.is_value_type() || t.is_enum_type()
                        );
                    }

                    // TODO: Is the offset supposed to be smaller than object size for fixups?
                    match t.is_value_type() && offset >= metadata.object_size() as u32 {
                        true => {
                            // value type fixup
                            offset - metadata.object_size() as u32
                        }
                        false => offset,
                    }
                }),
            }
        }

        fn get_size(
            field: &Il2CppFieldDefinition,
            gen_args: Option<&Vec<usize>>,
            metadata: &&Metadata<'_>,
        ) -> usize {
            let f_type = metadata
                .metadata_registration
                .types
                .get(field.type_index as usize)
                .unwrap();

            let sa = offsets::get_il2cpptype_sa(metadata, f_type, gen_args);

            sa.size
        }

        let fields = t
            .fields(metadata.metadata)
            .iter()
            .enumerate()
            .filter_map(|(i, field)| {
                let f_type = metadata
                    .metadata_registration
                    .types
                    .get(field.type_index as usize)
                    .unwrap();

                let field_index = FieldIndex::new(t.field_start.index() + i as u32);
                let f_name = field.name(metadata.metadata);

                let f_offset = get_offset(field, i, &mut offset_iter, field_offsets, metadata, t);

                // calculate / fetch the field size
                let f_size = get_size(field, self.generic_instantiations_args_types.as_ref(), &metadata);

                if let TypeData::TypeDefinitionIndex(field_tdi) = f_type.data
                    && metadata.blacklisted_types.contains(&field_tdi)
                {
                    if !self.is_value_type && !self.is_enum_type {
                        return None;
                    }
                    warn!("Value type uses {tdi:?} which is blacklisted! TODO");
                }

                // TODO: Check a flag to look for default values to speed this up
                let def_value = Self::field_default_value(metadata, field_index);

                assert!(def_value.is_none() || (def_value.is_some() && f_type.is_param_optional()));

                let cpp_field_decl = CsField {
                    name: f_name.to_owned(),
                    field_ty: f_type.data,
                    offset: f_offset,
                    instance: !f_type.is_static() && !f_type.is_constant(),
                    readonly: f_type.is_constant(),
                    brief_comment: Some(format!("Field {f_name}, offset: 0x{:x}, size: 0x{f_size:x}, def value: {def_value:?}", f_offset.unwrap_or(u32::MAX))),
                    value: def_value,
                    const_expr: false,
                };

                Some(FieldInfo {
                    cs_field: cpp_field_decl,
                    field,
                    field_type: f_type,
                    is_constant: f_type.is_constant(),
                    is_static: f_type.is_static(),
                    is_pointer: f_type.byref || !f_type.valuetype,
                    offset: f_offset,
                    size: f_size,
                })
            })
            .collect_vec();

        if t.is_value_type() || t.is_enum_type() {
            handle_valuetype_fields(self, &fields, metadata, tdi);
        } else {
            handle_referencetype_fields(self, &fields, metadata, tdi);
        }

        handle_static_fields(self, &fields, metadata, tdi);
        handle_const_fields(self, &fields, metadata, tdi);
    }

    fn make_parents(&mut self, metadata: &Metadata, tdi: TypeDefinitionIndex) {
        let t = &metadata.metadata.global_metadata.type_definitions[tdi];

        let ns = t.namespace(metadata.metadata);
        let name = t.name(metadata.metadata);

        if t.parent_index == u32::MAX {
            // TYPE_ATTRIBUTE_INTERFACE = 0x00000020
            match t.is_interface() {
                true => {
                    // FIXME: should interfaces have a base type? I don't think they need to
                    // self.inherit.push(INTERFACE_WRAPPER_TYPE.to_string());
                }
                false => {
                    info!("Skipping type: {ns}::{name} because it has parent index: {} and is not an interface!", t.parent_index);
                }
            }
            return;
        }

        let parent_type = metadata
            .metadata_registration
            .types
            .get(t.parent_index as usize)
            .unwrap_or_else(|| panic!("NO PARENT! But valid index found: {}", t.parent_index));

        let parent_ty: CsTypeTag = CsTypeTag::from_type_data(parent_type.data, metadata.metadata);

        // handle value types and enum types specially
        if !t.is_value_type() || t.is_enum_type() {
            // make sure our parent is intended\
            let is_ref_type = matches!(
                parent_type.ty,
                Il2CppTypeEnum::Class | Il2CppTypeEnum::Genericinst | Il2CppTypeEnum::Object
            );
            assert!(is_ref_type, "Not a class, object or generic inst!");

            self.parent = Some(parent_ty);
        }
    }

    fn make_interfaces(&mut self, metadata: &Metadata<'_>, tdi: TypeDefinitionIndex) {
        let t = &metadata.metadata.global_metadata.type_definitions[tdi];

        for &interface_index in t.interfaces(metadata.metadata) {
            let int_ty = &metadata.metadata_registration.types[interface_index as usize];

            let interface_tag = CsTypeTag::from_type_data(int_ty.data, metadata.metadata);
            self.interfaces.push(interface_tag);
        }
    }

    fn make_nested_types(&mut self, metadata: &Metadata, tdi: TypeDefinitionIndex) {
        let t = &metadata.metadata.global_metadata.type_definitions[tdi];

        if t.nested_type_count == 0 {
            return;
        }

        self.nested_types = t
            .nested_types(metadata.metadata)
            .iter()
            .map(|nested_tdi| {
                let _nested_td = &metadata.metadata.global_metadata.type_definitions[*nested_tdi];

                CsTypeTag::TypeDefinitionIndex(*nested_tdi)
            })
            .collect();
    }

    fn make_properties(&mut self, metadata: &Metadata, tdi: TypeDefinitionIndex) {
        let t = Self::get_type_definition(metadata, tdi);

        // Then, handle properties
        if t.property_count == 0 {
            return;
        }

        self.members.reserve(t.property_count as usize);
        // Then, for each field, write it out
        for prop in t.properties(metadata.metadata) {
            let p_name = prop.name(metadata.metadata);
            let p_setter = (prop.set != u32::MAX).then(|| prop.set_method(t, metadata.metadata));
            let p_getter = (prop.get != u32::MAX).then(|| prop.get_method(t, metadata.metadata));

            // if this is a static property, skip emitting a cpp property since those can't be static
            if p_getter.or(p_setter).unwrap().is_static_method() {
                continue;
            }

            let p_type_index = match p_getter {
                Some(g) => g.return_type as usize,
                None => p_setter.unwrap().parameters(metadata.metadata)[0].type_index as usize,
            };

            let p_type = metadata
                .metadata_registration
                .types
                .get(p_type_index)
                .unwrap();

            let _method_map = |p: MethodIndex| {
                let method_calc = metadata.method_calculations.get(&p).unwrap();
                CsMethodData {
                    estimated_size: method_calc.estimated_size,
                    addrs: method_calc.addrs,
                }
            };

            let _abstr = p_getter.is_some_and(|p| p.is_abstract_method())
                || p_setter.is_some_and(|p| p.is_abstract_method());

            let index = p_getter.is_some_and(|p| p.parameter_count > 0);

            // Need to include this type
            self.members.push(
                CsMember::Property(CsPropertyDecl {
                    name: p_name.to_owned(),
                    prop_ty: p_type.data,
                    // methods generated in make_methods
                    setter: p_setter.map(|m| m.name(metadata.metadata).to_string()),
                    getter: p_getter.map(|m| m.name(metadata.metadata).to_string()),
                    indexable: index,
                    brief_comment: None,
                    instance: true,
                })
                .into(),
            );
        }
    }

    pub fn create_method(
        &mut self,
        _declaring_type: &Il2CppTypeDefinition,
        method_index: MethodIndex,

        metadata: &Metadata,
        is_generic_method_inst: bool,
    ) {
        let method = &metadata.metadata.global_metadata.methods[method_index];

        // TODO: sanitize method name for c++
        let m_name = method.name(metadata.metadata);
        if m_name == ".cctor" {
            // info!("Skipping {}", m_name);
            return;
        }

        let m_ret_type = metadata
            .metadata_registration
            .types
            .get(method.return_type as usize)
            .unwrap();

        let m_params_with_def: Vec<CsParam> = self.make_parameters(method, metadata);

        let m_params_no_def: Vec<CsParam> = m_params_with_def
            .iter()
            .cloned()
            .map(|mut p| {
                p.def_value = None;
                p
            })
            .collect_vec();

        // TODO: Add template<typename ...> if a generic inst e.g
        // T UnityEngine.Component::GetComponent<T>() -> bs_hook::Il2CppWrapperType UnityEngine.Component::GetComponent()
        let template = method
            .generic_container_index
            .is_valid()
            .then(|| match is_generic_method_inst {
                true => Some(CsGenericTemplate { names: vec![] }),
                false => {
                    let generics = method
                        .generic_container(metadata.metadata)
                        .unwrap()
                        .generic_parameters(metadata.metadata)
                        .iter()
                        .map(|param| param.name(metadata.metadata).to_string());

                    Some(CsGenericTemplate::make_typenames(generics))
                }
            })
            .flatten();

        let _declaring_type_template = self
            .generic_template
            .as_ref()
            .is_some_and(|t| !t.names.is_empty())
            .then(|| self.generic_template.clone());

        let literal_types = is_generic_method_inst
            .then(|| {
                self.method_generic_instantiation_map
                    .get(&method_index)
                    .cloned()
            })
            .flatten();

        let _resolved_generic_types = literal_types.map(|literal_types| {
            literal_types
                .iter()
                .map(|t| &metadata.metadata_registration.types[*t as usize])
                .map(|t| CsTypeTag::from_type_data(t.data, metadata.metadata))
                .collect_vec()
        });

        let method_calc = metadata.method_calculations.get(&method_index);

        let mut method_decl = CsMethodDecl {
            brief: format!(
                "Method {m_name}, addr 0x{:x}, size 0x{:x}, virtual {}, abstract: {}, final {}",
                method_calc.map(|m| m.addrs).unwrap_or(u64::MAX),
                method_calc.map(|m| m.estimated_size).unwrap_or(usize::MAX),
                method.is_virtual_method(),
                method.is_abstract_method(),
                method.is_final_method()
            )
            .into(),
            name: m_name.to_string(),
            return_type: m_ret_type.data,
            parameters: m_params_no_def.clone(),
            instance: !method.is_static_method(),
            template: template.clone(),
            method_data: None,
        };

        // if type is a generic
        let has_template_args = self
            .generic_template
            .as_ref()
            .is_some_and(|t| !t.names.is_empty());

        // don't emit method size structs for generic methods
        if let Some(method_calc) = method_calc
            && template.is_none()
            && !has_template_args
            && !is_generic_method_inst
        {
            method_decl.method_data = Some(CsMethodData {
                addrs: method_calc.addrs,
                estimated_size: method_calc.estimated_size,
            })
        }

        if !is_generic_method_inst {
            self.members.push(CsMember::MethodDecl(method_decl).into());
        }
    }

    fn default_value_blob(
        metadata: &Metadata,
        ty: &Il2CppType,
        data_index: usize,
        _string_quotes: bool,
        _string_as_u16: bool,
    ) -> CsValue {
        let data = &metadata
            .metadata
            .global_metadata
            .field_and_parameter_default_value_data
            .as_vec()[data_index..];

        let mut cursor = Cursor::new(data);

        match ty.ty {
            Il2CppTypeEnum::Boolean => CsValue::Bool(data[0] != 0),
            Il2CppTypeEnum::I1 => CsValue::I8(cursor.read_i8().unwrap()),
            Il2CppTypeEnum::I2 => CsValue::I16(cursor.read_i16::<Endian>().unwrap()),
            Il2CppTypeEnum::I4 => CsValue::I32(cursor.read_compressed_i32::<Endian>().unwrap()),
            // TODO: We assume 64 bit
            Il2CppTypeEnum::I | Il2CppTypeEnum::I8 => {
                CsValue::I64(cursor.read_i64::<Endian>().unwrap())
            }
            Il2CppTypeEnum::U1 => CsValue::U8(cursor.read_u8().unwrap()),
            Il2CppTypeEnum::U2 => CsValue::U16(cursor.read_u16::<Endian>().unwrap()),
            Il2CppTypeEnum::U4 => CsValue::U32(cursor.read_u32::<Endian>().unwrap()),
            // TODO: We assume 64 bit
            Il2CppTypeEnum::U | Il2CppTypeEnum::U8 => {
                CsValue::U64(cursor.read_u64::<Endian>().unwrap())
            }
            // https://learn.microsoft.com/en-us/nimbusml/concepts/types
            // https://en.cppreference.com/w/cpp/types/floating-point
            Il2CppTypeEnum::R4 => CsValue::F32(cursor.read_f32::<Endian>().unwrap()),
            Il2CppTypeEnum::R8 => CsValue::F64(cursor.read_f64::<Endian>().unwrap()),
            Il2CppTypeEnum::Char => {
                let res = String::from_utf16_lossy(&[cursor.read_u16::<Endian>().unwrap()])
                    .escape_default()
                    .to_string();

                CsValue::String(res)
            }
            Il2CppTypeEnum::String => {
                let stru16_len = cursor.read_compressed_i32::<Endian>().unwrap();
                if stru16_len == -1 {
                    return CsValue::String("".to_string());
                }

                let mut buf = vec![0u8; stru16_len as usize];

                cursor.read_exact(buf.as_mut_slice()).unwrap();

                let res = String::from_utf8(buf).unwrap().escape_default().to_string();

                CsValue::String(res)
            }
            Il2CppTypeEnum::Genericinst
            | Il2CppTypeEnum::Byref
            | Il2CppTypeEnum::Ptr
            | Il2CppTypeEnum::Array
            | Il2CppTypeEnum::Object
            | Il2CppTypeEnum::Class
            | Il2CppTypeEnum::Valuetype
            | Il2CppTypeEnum::Szarray => {
                // let def = Self::type_default_value(metadata, None, ty);
                // format!("/* TODO: Fix these default values */ {ty:?} */ {def}")
                CsValue::Null
            }

            _ => todo!("Unsupported blob type {:#?}", ty),
        }
    }

    fn unbox_nullable_valuetype<'a>(metadata: &'a Metadata, ty: &'a Il2CppType) -> &'a Il2CppType {
        if let Il2CppTypeEnum::Valuetype = ty.ty {
            match ty.data {
                TypeData::TypeDefinitionIndex(tdi) => {
                    let type_def = &metadata.metadata.global_metadata.type_definitions[tdi];

                    // System.Nullable`1
                    if type_def.name(metadata.metadata) == "Nullable`1"
                        && type_def.namespace(metadata.metadata) == "System"
                    {
                        return metadata
                            .metadata_registration
                            .types
                            .get(type_def.byval_type_index as usize)
                            .unwrap();
                    }
                }
                _ => todo!(),
            }
        }

        ty
    }

    fn field_default_value(metadata: &Metadata, field_index: FieldIndex) -> Option<CsValue> {
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
                    return CsValue::Null;
                }

                Self::default_value_blob(metadata, ty, def.data_index.index() as usize, true, true)
            })
    }
    fn param_default_value(
        metadata: &Metadata,
        parameter_index: ParameterIndex,
    ) -> Option<CsValue> {
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

                ty = Self::unbox_nullable_valuetype(metadata, ty);

                // This occurs when the type is `null` or `default(T)` for value types
                if !def.data_index.is_valid() {
                    return CsValue::Null;
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

                Self::default_value_blob(metadata, ty, def.data_index.index() as usize, true, true)
            })
    }

    pub fn get_type_definition<'a>(
        metadata: &'a Metadata,
        tdi: TypeDefinitionIndex,
    ) -> &'a Il2CppTypeDefinition {
        &metadata.metadata.global_metadata.type_definitions[tdi]
    }
}
