use std::collections::HashSet;

use brocolib::runtime_metadata::Il2CppTypeEnum;
use color_eyre::eyre::{Context, ContextCompat, Result};
use itertools::Itertools;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::parse_quote;

use crate::{
    data::{
        name_components::NameComponents,
        type_resolver::{ResolvedType, ResolvedTypeData, TypeUsage},
    },
    generate::{
        cs_members::{CsConstructor, CsField, CsMethod, CsParam},
        cs_type::CsType,
        cs_type_tag::CsTypeTag,
        metadata::CordlMetadata,
        offsets::SizeInfo,
        type_extensions::{TypeDefinitionExtensions, TypeDefinitionIndexExtensions},
        writer::{Writable, Writer},
    },
};

use super::{
    config::RustGenerationConfig,
    rust_fields,
    rust_members::{ConstRustField, RustField, RustFunction, RustParam, RustTrait, Visibility},
    rust_name_components::RustNameComponents,
    rust_name_resolver::RustNameResolver,
};

use std::io::Write;

const PARENT_FIELD: &str = "__cordl_parent";

#[derive(Clone, Debug, Default)]
pub struct RustTypeRequirements {
    required_modules: HashSet<String>,
}

impl RustTypeRequirements {
    pub fn add_module(&mut self, module: &str) {
        self.required_modules.insert(module.to_string());
    }

    pub(crate) fn needs_object_include(&mut self) {
        self.add_module("quest_hook::libil2cpp::Il2CppObject");
    }

    pub(crate) fn needs_array_include(&mut self) {
        self.add_module("quest_hook::libil2cpp::Il2CppArray");
    }

    pub(crate) fn needs_string_include(&mut self) {
        self.add_module("quest_hook::libil2cpp::Il2CppString");
    }

    pub(crate) fn needs_byref_include(&mut self) {
        // todo!()
    }

    pub(crate) fn needs_byref_const_include(&mut self) {
        // todo!()
    }

    pub(crate) fn get_modules(&self) -> &HashSet<String> {
        &self.required_modules
    }
}

#[derive(Clone)]
pub struct RustType {
    // TODO: union
    pub fields: Vec<RustField>,
    pub constants: Vec<ConstRustField>,
    pub methods: Vec<RustFunction>,
    pub traits: Vec<RustTrait>,
    pub nested_types: Vec<syn::ItemType>,

    pub is_value_type: bool,
    pub is_enum_type: bool,
    pub is_reference_type: bool,
    pub is_interface: bool,

    pub self_tag: CsTypeTag,

    pub parent: Option<RustNameComponents>,
    pub backing_type_enum: Option<RustNameComponents>,

    pub generics: Option<Vec<String>>,
    pub cs_name_components: NameComponents,
    pub rs_name_components: RustNameComponents,
    pub(crate) prefix_comments: Vec<String>,

    pub requirements: RustTypeRequirements,
    pub packing: Option<u32>,
    pub size_info: Option<SizeInfo>,
    pub is_compiler_generated: bool,
}
impl RustType {
    pub(crate) fn make_rust_type(
        tag: CsTypeTag,
        cs_type: &CsType,
        config: &RustGenerationConfig,
    ) -> Self {
        let cs_name_components = &cs_type.cs_name_components;

        let rs_name_components = RustNameComponents {
            generics: cs_name_components.generics.clone(),
            name: config.name_rs(&cs_name_components.name),
            namespace: cs_name_components
                .namespace
                .as_ref()
                .map(|s| config.namespace_rs(s)),
            is_ref: false,
            is_ptr: cs_type.is_reference_type,
            is_mut: cs_type.is_reference_type, // value types don't need to be mutable
        };

        RustType {
            fields: Default::default(),
            methods: Default::default(),
            traits: Default::default(),
            constants: Default::default(),
            nested_types: Default::default(),

            is_value_type: cs_type.is_value_type,
            is_enum_type: cs_type.is_enum_type,
            is_reference_type: cs_type.is_reference_type,
            is_interface: cs_type.is_interface,
            parent: Default::default(),
            backing_type_enum: Default::default(),

            requirements: RustTypeRequirements::default(),
            self_tag: tag,
            generics: cs_type
                .generic_template
                .as_ref()
                .map(|g| g.just_names().cloned().collect_vec()),

            rs_name_components,
            cs_name_components: cs_type.cs_name_components.clone(),
            prefix_comments: vec![],
            packing: cs_type.packing.map(|p| p as u32),
            size_info: cs_type.size_info.clone(),
            is_compiler_generated: cs_type.is_compiler_generated,
        }
    }

    pub fn fill(
        &mut self,
        cs_type: CsType,
        name_resolver: &RustNameResolver,
        config: &RustGenerationConfig,
    ) {
        self.make_parent(cs_type.parent.as_ref(), name_resolver);
        self.make_nested_types(&cs_type.nested_types, name_resolver);
        self.make_interfaces(&cs_type.interfaces, name_resolver, config);

        self.make_fields(&cs_type.fields, name_resolver, config);

        self.make_instance_methods(&cs_type.methods, name_resolver, config);
        self.make_static_methods(&cs_type.methods, name_resolver, config);

        if self.is_reference_type {
            self.make_ref_constructors(&cs_type.constructors, name_resolver, config);
        }

        if let Some(backing_type) = cs_type.enum_backing_type {
            let backing_ty = RustNameResolver::primitive_to_rust_ty(&backing_type);
            let resolved_ty = RustNameComponents {
                name: backing_ty.to_owned(),
                namespace: None,
                generics: None,
                is_ref: false,
                is_ptr: false,
                is_mut: false,
            };

            self.backing_type_enum = Some(resolved_ty);
        }
    }

    fn make_parent(
        &mut self,
        parent: Option<&ResolvedType>,
        name_resolver: &RustNameResolver<'_, '_>,
    ) {
        if self.is_value_type || self.is_enum_type {
            return;
        }

        let Some(parent) = parent else { return };
        let parent = name_resolver.resolve_name(self, parent, TypeUsage::TypeName, true);
        let parent_field = RustField {
            name: format_ident!("{}", PARENT_FIELD),
            field_type: parent.to_type_token(),
            visibility: Visibility::Private,
            offset: 0,
        };

        self.fields.insert(0, parent_field);
        self.parent = Some(parent);
    }

    fn make_nested_types(
        &mut self,
        nested_types: &HashSet<CsTypeTag>,
        name_resolver: &RustNameResolver<'_, '_>,
    ) {
        let nested_types = nested_types
            .iter()
            .filter_map(|tag| name_resolver.collection.get_cpp_type(*tag))
            .map(|rust_type| -> syn::ItemType {
                let name = format_ident!(
                    "{}",
                    name_resolver
                        .config
                        .name_rs(&rust_type.cs_name_components.name)
                );

                let target = rust_type.rs_name_components.to_type_path_token();

                let visibility = match rust_type.is_interface {
                    false => Visibility::Public,
                    true => Visibility::Private,
                }
                .to_token_stream();

                parse_quote! {
                    #visibility type #name = #target;
                }
            });

        self.nested_types = nested_types.collect();
    }

    fn make_fields(
        &mut self,
        fields: &[CsField],
        name_resolver: &RustNameResolver,
        config: &RustGenerationConfig,
    ) {
        let instance_fields = fields
            .iter()
            .filter(|f| f.instance && !f.is_const)
            .cloned()
            .collect_vec();

        if self.is_value_type && !self.is_enum_type {
            rust_fields::handle_valuetype_fields(self, &instance_fields, name_resolver, config);
        } else {
            rust_fields::handle_referencetype_fields(self, &instance_fields, name_resolver, config);
        }

        rust_fields::handle_static_fields(self, fields, name_resolver, config);
        rust_fields::handle_const_fields(self, fields, name_resolver, config);

        // for f in fields {
        //     if !f.instance || f.is_const {
        //         continue;
        //     }
        //     let field_type = name_resolver.resolve_name(self, &f.field_ty, TypeUsage::Field, true);

        //     let rust_field = RustField {
        //         name: config.name_rs(&f.name),
        //         field_type: RustItem::NamedType(field_type.combine_all()),
        //         visibility: Visibility::Public,
        //         offset: f.offset.unwrap_or_default(),
        //     };
        //     self.fields.push(rust_field);
        // }
    }

    fn make_interfaces(
        &mut self,
        interfaces: &[ResolvedType],
        name_resolver: &RustNameResolver,
        config: &RustGenerationConfig,
    ) {
        for i in interfaces {
            let interface = name_resolver.resolve_name(self, i, TypeUsage::TypeName, true);
            let rust_trait = RustTrait {
                ty: interface.to_type_path_token(),
            };
            self.traits.push(rust_trait);
        }
    }

    fn make_ref_constructors(
        &mut self,
        constructors: &[CsConstructor],
        name_resolver: &RustNameResolver<'_, '_>,
        config: &RustGenerationConfig,
    ) {
        for c in constructors {
            let params = c
                .parameters
                .iter()
                .map(|p| self.make_parameter(p, name_resolver, config))
                .collect_vec();

            let param_names = params.iter().map(|p| &p.name);

            let body: Vec<syn::Stmt> = parse_quote! {
                let object: &mut Self = Self::class().instantiate();

                (object as &mut Il2CppObject).invoke_void(".ctor", (#(#param_names),*))?;

                Ok(object)
            };

            let rust_func = RustFunction {
                name: format_ident!("New"),
                body: Some(body),

                is_mut: true,
                is_ref: true,
                is_self: false,
                params,

                return_type: Some(parse_quote!(quest_hook::Result<&'static mut Self>)),
                visibility: (Visibility::Public),
            };
            self.methods.push(rust_func);
        }

        //         pub fn new() -> &'static mut Self {
        //     let object: &mut Self = Self::class().instantiate();

        //     object.object.invoke_void(".ctor", ()).unwrap();

        //     object
        // }
    }

    fn make_instance_methods(
        &mut self,
        methods: &[CsMethod],
        name_resolver: &RustNameResolver,
        config: &RustGenerationConfig,
    ) {
        for (_, overload_methods) in methods
            .iter()
            .filter(|m| m.instance)
            .into_group_map_by(|m| &m.name)
        {
            for m in &overload_methods {
                let m_name = &m.name;
                let mut m_name_rs = config.name_rs(m_name);

                if overload_methods.len() > 1 {
                    m_name_rs = format!(
                        "{}{}",
                        m_name_rs,
                        m.parameters
                            .iter()
                            .map(|p| name_resolver
                                .resolve_name(self, &p.il2cpp_ty, TypeUsage::Parameter, true)
                                .name)
                            .join("_")
                    );
                }

                let m_ret_ty = name_resolver
                    .resolve_name(self, &m.return_type, TypeUsage::ReturnType, true)
                    .to_type_token();

                let params = m
                    .parameters
                    .iter()
                    .map(|p| self.make_parameter(p, name_resolver, config))
                    .collect_vec();

                let param_names = params.iter().map(|p| &p.name);

                let body: Vec<syn::Stmt> = if m.return_type.data
                    == ResolvedTypeData::Primitive(Il2CppTypeEnum::Void)
                {
                    parse_quote! {
                        (self as &mut Il2CppObject).invoke_void(#m_name, ( #(#param_names),* ) )?;
                        Ok(())
                    }
                } else {
                    parse_quote! {
                        let ret: #m_ret_ty = (self as &mut Il2CppObject).invoke(#m_name, ( #(#param_names),* ) )?;

                        Ok(ret)
                    }
                };

                let rust_func = RustFunction {
                    name: format_ident!("{m_name_rs}"),
                    body: Some(body),

                    is_mut: true,
                    is_ref: true,
                    is_self: m.instance,
                    params,

                    return_type: Some(m_ret_ty),
                    visibility: (Visibility::Public),
                };
                self.methods.push(rust_func);
            }
        }
    }

    fn make_static_methods(
        &mut self,
        methods: &[CsMethod],
        name_resolver: &RustNameResolver,
        config: &RustGenerationConfig,
    ) {
        for (_, overload_methods) in methods
            .iter()
            .filter(|m| !m.instance)
            .into_group_map_by(|m| &m.name)
        {
            for m in &overload_methods {
                let m_name = &m.name;
                let mut m_name_rs = config.name_rs(m_name);

                if overload_methods.len() > 1 {
                    m_name_rs = format!(
                        "{}{}",
                        m_name,
                        m.parameters
                            .iter()
                            .map(|p| name_resolver
                                .resolve_name(self, &p.il2cpp_ty, TypeUsage::Parameter, true)
                                .name)
                            .join("_")
                    );
                }

                let m_ret_ty = name_resolver
                    .resolve_name(self, &m.return_type, TypeUsage::ReturnType, true)
                    .to_type_token();

                let params = m
                    .parameters
                    .iter()
                    .map(|p| self.make_parameter(p, name_resolver, config))
                    .collect_vec();

                let param_names = params.iter().map(|p| &p.name);

                let body: Vec<syn::Stmt> = if m.return_type.data
                    == ResolvedTypeData::Primitive(Il2CppTypeEnum::Void)
                {
                    parse_quote! {
                        Self::class().invoke_void(#m_name, ( #(#param_names),* ) )?;
                        Ok(())
                    }
                } else {
                    parse_quote! {
                        let ret: #m_ret_ty = Self::class().invoke_void(#m_name, ( #(#param_names),* ) );

                        Ok(ret)
                    }
                };

                let rust_func = RustFunction {
                    name: format_ident!("{m_name_rs}"),
                    body: Some(body),

                    is_mut: false,
                    is_ref: false,
                    is_self: false,
                    params,

                    return_type: Some(m_ret_ty),
                    visibility: (Visibility::Public),
                };
                self.methods.push(rust_func);
            }
        }
    }

    fn make_parameter(
        &mut self,
        p: &CsParam,
        name_resolver: &RustNameResolver<'_, '_>,
        config: &RustGenerationConfig,
    ) -> RustParam {
        let p_ty = name_resolver.resolve_name(self, &p.il2cpp_ty, TypeUsage::Field, true);
        // let p_il2cpp_ty = p.il2cpp_ty.get_type(name_resolver.cordl_metadata);

        let name_rs = config.name_rs(&p.name);
        RustParam {
            name: format_ident!("{name_rs}"),
            param_type: p_ty.to_type_token(),
            // is_ref: p_il2cpp_ty.is_byref(),
            // is_ptr: !p_il2cpp_ty.valuetype,
            // is_mut: true,
        }
    }

    pub fn name(&self) -> &String {
        &self.cs_name_components.name
    }

    pub fn namespace(&self) -> Option<&str> {
        self.cs_name_components.namespace.as_deref()
    }

    pub fn rs_name(&self) -> &String {
        &self.rs_name_components.name
    }
    pub fn rs_namespace(&self) -> &Option<String> {
        &self.rs_name_components.namespace
    }

    pub(crate) fn write(&self, writer: &mut Writer, config: &RustGenerationConfig) -> Result<()> {
        if self.is_value_type {
            if self.is_enum_type {
                self.write_enum_type(writer, config)?;
            } else {
                self.write_value_type(writer, config)?;
            }
        }

        if self.is_interface {
            self.write_interface(writer, config)?;
        } else if self.is_reference_type {
            self.write_reference_type(writer, config)?;
        }

        Ok(())
    }

    pub fn nested_fixup(
        &mut self,
        cs_type: &CsType,
        metadata: &CordlMetadata,
        config: &RustGenerationConfig,
    ) {
        // Nested type unnesting fix
        let Some(declaring_tag) = cs_type.declaring_ty.as_ref() else {
            return;
        };

        let declaring_td = declaring_tag
            .get_tdi()
            .get_type_definition(metadata.metadata);

        let declaring_name = declaring_td.get_name_components(metadata.metadata).name;
        let declaring_namespace = declaring_td.namespace(metadata.metadata);

        let combined_name = format!("{}_{}", declaring_name, self.name());

        self.rs_name_components.namespace = Some(config.namespace_rs(declaring_namespace));
        self.rs_name_components.name = config.name_rs(&combined_name);
    }

    fn write_reference_type(
        &self,
        writer: &mut Writer,
        config: &RustGenerationConfig,
    ) -> Result<()> {
        let name_ident = self.rs_name_components.to_name_ident();

        let fields = self.fields.iter().map(|f| {
            let f_name = format_ident!(r#"{}"#, f.name);
            let f_ty = &f.field_type;
            let f_visibility = match f.visibility {
                Visibility::Public => quote! { pub },
                Visibility::PublicCrate => quote! { pub(crate) },
                Visibility::Private => quote! {},
            };

            quote! {
                #f_visibility #f_name: #f_ty
            }
        });

        let cs_name_str = self.cs_name_components.combine_all();

        let quest_hook_path: syn::Path = parse_quote!(quest_hook::libil2cpp);
        let macro_invoke: syn::ItemMacro = parse_quote! {
            #quest_hook_path::unsafe_impl_reference_type!(in #quest_hook_path for #name_ident => #cs_name_str);
        };

        let mut tokens = quote! {
            #[repr(c)]
            #[derive(Debug)]
            pub struct #name_ident {
                #(#fields),*
            }
            #macro_invoke
        };

        // example of using the il2cpp_subtype macro
        // il2cpp_subtype!(List, Il2CppObject, object);
        // macro_rules! il2cpp_subtype {
        //     ($type:ident, $target:ty, $field:ident) => {
        //         impl<T: Type> std::ops::Deref for $type<T> {
        //             type Target = $target;

        //             fn deref(&self) -> &Self::Target {
        //                 &self.$field
        //             }
        //         }

        //         impl<T: Type> std::ops::DerefMut for $type<T> {
        //             fn deref_mut(&mut self) -> &mut Self::Target {
        //                 &mut self.$field
        //             }
        //         }
        //     };
        // }
        // il2cpp_subtype!(List<T>, Il2CppObject, object);
        if let Some(parent) = &self.parent {
            let parent_name = parent.clone().to_type_path_token();
            let parent_field_ident = format_ident!(r#"{}"#, PARENT_FIELD);

            tokens.extend(quote! {
                    quest_hook::libil2cpp::il2cpp_subtype!(#name_ident => #parent_name, #parent_field_ident);
                });
        }

        writer.write_pretty_tokens(tokens)?;

        self.write_impl(writer, config)?;
        Ok(())
    }

    fn write_enum_type(&self, writer: &mut Writer, config: &RustGenerationConfig) -> Result<()> {
        let fields = self.constants.iter().map(|f| -> syn::Variant {
            let name = &f.name;
            let val = &f.value;

            parse_quote! {
                #name = #val
            }
        });
        let backing_type = self
            .backing_type_enum
            .as_ref()
            .wrap_err("No enum backing type found!")?
            .to_type_token();

        let name_ident = self.rs_name_components.to_name_ident();

        let cs_name_str = self.cs_name_components.combine_all();

        let quest_hook_path: syn::Path = parse_quote!(quest_hook::libil2cpp);
        let macro_invoke: syn::ItemMacro = parse_quote! {
            #quest_hook_path::unsafe_impl_reference_type!(in #quest_hook_path for #name_ident => #cs_name_str);
        };

        let tokens = quote! {
            #[repr(#backing_type)]
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub enum #name_ident {
                #(#fields),*
            }
            #macro_invoke
        };

        writer.write_pretty_tokens(tokens)?;

        self.write_impl(writer, config)?;

        Ok(())
    }

    fn write_value_type(&self, writer: &mut Writer, config: &RustGenerationConfig) -> Result<()> {
        let name_ident = self.rs_name_components.to_name_ident();

        let fields = self.fields.iter().map(|f| {
            let f_name = format_ident!(r#"{}"#, f.name);
            let f_ty = &f.field_type;
            let f_visibility = match f.visibility {
                Visibility::Public => quote! { pub },
                Visibility::PublicCrate => quote! { pub(crate) },
                Visibility::Private => quote! {},
            };

            quote! {
                #f_visibility #f_name: #f_ty
            }
        });

        let cs_name_str = self.cs_name_components.combine_all();

        let quest_hook_path: syn::Path = parse_quote!(quest_hook::libil2cpp);
        let macro_invoke: syn::ItemMacro = parse_quote! {
            #quest_hook_path::unsafe_impl_value_type!(in #quest_hook_path for #name_ident => #cs_name_str);
        };

        let mut tokens = quote! {
            #[repr(c)]
            #[derive(Debug, Clone)]
            pub struct #name_ident {
                #(#fields),*
            }
            #macro_invoke
        };

        writer.write_pretty_tokens(tokens)?;

        self.write_impl(writer, config)?;

        Ok(())
    }

    fn write_impl(&self, writer: &mut Writer, _config: &RustGenerationConfig) -> Result<()> {
        let name_ident = self.rs_name_components.clone().to_name_ident();

        let generics = self
            .generics
            .as_ref()
            .map(|g| {
                g.iter()
                    .map(|g| -> syn::GenericArgument { syn::parse_str(g).unwrap() })
                    .collect_vec()
            })
            .map(|g| -> syn::Generics {
                parse_quote! { <#(#g),*> }
            });

        let const_fields = self.constants.iter().map(|f| -> syn::ImplItemConst {
            let name = &f.name;
            let val = &f.value;
            let f_ty = &f.field_type;

            parse_quote! {
                pub const #name: #f_ty = #val;
            }
        });

        let methods = self
            .methods
            .iter()
            .cloned()
            .map(|mut f| {
                f.body = f.body.or(Some(parse_quote! {
                    todo!()
                }));
                f
            })
            .map(|f| f.to_token_stream())
            .map(|f| -> syn::ImplItemFn { parse_quote!(#f) });

        let nested_types = &self.nested_types;

        let other_impls = self
            .traits
            .iter()
            .map(|t| -> syn::ItemImpl {
                let ty = &t.ty;

                match generics.as_ref() {
                    Some(generics) => {
                        parse_quote! {
                            impl #generics #ty for #name_ident {}
                        }
                    }
                    None => {
                        parse_quote! {
                            impl #ty for #name_ident {}
                        }
                    }
                }
            })
            .collect_vec();

        let impl_tokens: syn::ItemImpl = match generics {
            Some(generics) => {
                parse_quote! {
                    impl #generics #name_ident {
                        #(#const_fields)*
                        #(#nested_types)*
                        #(#methods)*
                    }
                }
            }
            None => {
                parse_quote! {
                    impl #name_ident {
                        #(#const_fields)*
                        #(#nested_types)*
                        #(#methods)*
                    }
                }
            }
        };

        let tokens = quote! {
            #impl_tokens
            #(#other_impls)*
        };

        writer.write_pretty_tokens(tokens.to_token_stream())?;
        Ok(())
    }

    fn write_interface(&self, writer: &mut Writer, _config: &RustGenerationConfig) -> Result<()> {
        let name_ident = self.rs_name_components.to_name_ident();
        let methods = self
            .methods
            .iter()
            .cloned()
            .map(|mut f| {
                f.body = f.body.or(Some(parse_quote! {
                    todo!()
                }));
                f.visibility = Visibility::Private;
                f
            })
            .map(|f| f.to_token_stream())
            .map(|f| -> syn::ImplItemFn { parse_quote!(#f) });

        let cs_name_str = self.cs_name_components.combine_all();

        let quest_hook_path: syn::Path = parse_quote!(quest_hook::libil2cpp);
        let macro_invoke: syn::ItemMacro = parse_quote! {
            #quest_hook_path::unsafe_impl_reference_type!(in #quest_hook_path for #name_ident => #cs_name_str);
        };

        let tokens = quote! {
            pub trait #name_ident: libil2cpp::typecheck::ty::Type {
                #(#methods)*
            }

            #macro_invoke
        };

        writer.write_pretty_tokens(tokens)?;

        Ok(())
    }

    pub(crate) fn classof_name(&self) -> String {
        format!("{}::class()", self.rs_name())
    }
}

impl Writer {
    pub(crate) fn write_pretty_tokens(&mut self, tokens: TokenStream) -> Result<()> {
        let syntax_tree = syn::parse2(tokens.clone()).with_context(|| format!("{tokens}"))?;
        let formatted = prettyplease::unparse(&syntax_tree);

        self.stream.write_all(formatted.as_bytes())?;
        Ok(())
    }
}
