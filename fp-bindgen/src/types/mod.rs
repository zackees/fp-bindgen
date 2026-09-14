use crate::primitives::Primitive;
use std::{collections::BTreeMap, hash::Hash};
use syn::{Item, TypeParam, TypeParamBound};

mod cargo_dependency;
mod custom_type;
mod enums;
mod structs;
mod type_ident;

pub use cargo_dependency::CargoDependency;
pub use custom_type::CustomType;
pub use enums::{Enum, EnumOptions, Variant, VariantAttrs};
pub use structs::{Field, FieldAttrs, Struct, StructOptions};
pub use type_ident::TypeIdent;

/// A nominal opaque capability handle owned by the host runtime.
///
/// A resource is not a serializable value. Backends must lower it through a
/// versioned handle ABI and preserve its host-owned lifecycle.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Resource {
    pub ident: TypeIdent,
}

impl Resource {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            ident: TypeIdent::from(name.into()),
        }
    }
}

pub type TypeMap = BTreeMap<TypeIdent, Type>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    Alias(String, TypeIdent),
    Array(Primitive, usize),
    Container(String, TypeIdent),
    Custom(CustomType),
    Enum(Enum),
    List(String, TypeIdent),
    Map(String, TypeIdent, TypeIdent),
    Primitive(Primitive),
    Resource(Resource),
    String,
    Struct(Struct),
    Tuple(Vec<TypeIdent>),
    Unit,
}

impl Type {
    pub fn from_item(item_str: &str) -> Self {
        let item = syn::parse_str::<Item>(item_str).unwrap();
        match item {
            Item::Enum(item) => Type::Enum(enums::parse_enum_item(item)),
            Item::Struct(item) => Type::Struct(structs::parse_struct_item(item)),
            item => panic!(
                "Only struct and enum types can be constructed from an item. Found: {:?}",
                item
            ),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Self::Alias(name, _) => name.clone(),
            Self::Array(primitive, size) => format!("[{}; {}]", primitive.name(), size),
            Self::Container(name, ident) => format!("{name}<{ident}>"),
            Self::Custom(custom) => custom.ident.to_string(),
            Self::Enum(Enum { ident, .. }) => ident.to_string(),
            Self::List(name, ident) => format!("{name}<{ident}>"),
            Self::Map(name, key, value) => format!("{name}<{key}, {value}>"),
            Self::Primitive(primitive) => primitive.name(),
            Self::Resource(resource) => resource.ident.to_string(),
            Self::String => "String".to_owned(),
            Self::Struct(Struct { ident, .. }) => ident.to_string(),
            Self::Tuple(items) => format!(
                "({})",
                items
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Unit => "()".to_owned(),
        }
    }
}

pub(crate) fn format_bounds(ty: &TypeParam) -> Vec<String> {
    ty.bounds
        .iter()
        .filter_map(|bound| match bound {
            TypeParamBound::Trait(tr) => Some(path_to_string(&tr.path)),
            _ => None,
        })
        .collect()
}

fn path_to_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

// Used to remove the 'Serializable' bound from generated types, since this trait only exists in fp-bindgen
// and doesn't exist at runtime.
pub(crate) fn is_runtime_bound(bound: &str) -> bool {
    // Filtering by string is a bit dangerous since users may have their own 'Serializable' trait :(
    bound != "Serializable" && bound != "fp_bindgen::prelude::Serializable"
}
