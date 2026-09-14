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
    /// Whether this declaration transfers an owned, host-issued capability
    /// into guest code.  The Core Wasm backend gives these handles a release
    /// control and never treats their integer representation as a value.
    pub ownership: ResourceOwnership,
}

/// The ownership contract of an opaque host resource.
///
/// `Transport` preserves the original nominal-handle lowering for protocols
/// which already own their lifecycle. `Owned` asks a backend to generate an
/// explicit release path; it must not be lowered by value generators.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResourceOwnership {
    Transport,
    Owned,
}

impl Resource {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            ident: TypeIdent::from(name.into()),
            ownership: ResourceOwnership::Transport,
        }
    }

    /// Declares a host-owned capability whose generated guest handle releases
    /// itself exactly once, either explicitly or on normal Rust drop.
    ///
    /// Store/trap teardown and generation validation remain host policy: the
    /// generated release import delegates atomically to the consuming host's
    /// canonical resource registry.
    pub fn owned(name: impl Into<String>) -> Self {
        Self {
            ident: TypeIdent::from(name.into()),
            ownership: ResourceOwnership::Owned,
        }
    }

    pub fn is_owned(&self) -> bool {
        self.ownership == ResourceOwnership::Owned
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
