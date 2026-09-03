use crate::syntax::{BaseType, RefinementType};
use std::collections::{BTreeMap, BTreeSet};

use super::Environment;

/// One parameter in an ambient library signature.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryParameter {
    pub name: String,
    pub ty: RefinementType,
    pub optional: bool,
    pub rest: bool,
}

impl LibraryParameter {
    pub fn required(name: impl Into<String>, ty: RefinementType) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: false,
            rest: false,
        }
    }

    pub fn optional(name: impl Into<String>, ty: RefinementType) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: true,
            rest: false,
        }
    }

    pub fn rest(name: impl Into<String>, ty: RefinementType) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: false,
            rest: true,
        }
    }
}

/// How a native function uses a callback parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackTiming {
    /// The callback is invoked before the native call returns.
    Immediate,
    /// The callback may be invoked after the native call returns.
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallbackUse {
    pub parameter_index: usize,
    pub timing: CallbackTiming,
}

/// Receiver and ambient-state effects needed to invalidate stale refinements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverEffect {
    None,
    Read,
    Mutate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionEffects {
    pub receiver: ReceiverEffect,
    pub callbacks: Vec<CallbackUse>,
    /// The host call can synchronously re-enter arbitrary user code without
    /// receiving that code as an explicit callback parameter.
    pub executes_user_code: bool,
    pub writes_ambient_state: bool,
}

impl Default for FunctionEffects {
    fn default() -> Self {
        Self {
            receiver: ReceiverEffect::None,
            callbacks: Vec::new(),
            executes_user_code: false,
            writes_ambient_state: false,
        }
    }
}

/// Semantic facts which cannot yet be represented by `PredicateExpr` alone.
///
/// The base signature remains useful without interpreting these rules. A
/// refinement-aware checker can opt into them explicitly instead of inferring
/// facts from a function's spelling.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticRefinement {
    TypeGuard {
        parameter_index: usize,
        narrowed_to: BaseType,
    },
    ResultLengthEqualsReceiver,
    ResultLengthAtMostReceiver,
    ResultElementsFromCallback {
        callback_parameter: usize,
    },
    ResultElementsSubsetOfReceiver,
    ReceiverLengthIncreasesByArgumentCount,
    /// Dense-array `pop`: the receiver must have a positive length index.
    RequiresPositiveReceiverLength,
    /// Dense-array `pop`: length becomes `n - 1` after the call.
    ReceiverLengthDecreasesByOne,
    /// Every reference argument may become reachable through the receiver.
    ReceiverMayContainArguments,
}

/// One callable overload. Type parameter names include their `$` prefix, and
/// references use `BaseType::Named("$T")`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    pub type_parameters: Vec<String>,
    pub receiver: Option<RefinementType>,
    pub parameters: Vec<LibraryParameter>,
    pub returns: RefinementType,
    pub effects: FunctionEffects,
    pub refinements: Vec<SemanticRefinement>,
}

impl FunctionSignature {
    pub fn new(parameters: Vec<LibraryParameter>, returns: RefinementType) -> Self {
        Self {
            type_parameters: Vec::new(),
            receiver: None,
            parameters,
            returns,
            effects: FunctionEffects::default(),
            refinements: Vec::new(),
        }
    }

    pub fn with_type_parameters(mut self, parameters: &[&str]) -> Self {
        assert!(
            parameters.iter().all(|name| name.starts_with('$')),
            "library type parameters must use the '$' prefix"
        );
        self.type_parameters = parameters.iter().map(|name| (*name).to_string()).collect();
        self
    }

    pub fn with_receiver(mut self, receiver: RefinementType) -> Self {
        self.receiver = Some(receiver);
        self
    }

    pub fn with_effects(mut self, effects: FunctionEffects) -> Self {
        self.effects = effects;
        self
    }

    pub fn with_refinements(mut self, refinements: Vec<SemanticRefinement>) -> Self {
        self.refinements = refinements;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertySignature {
    pub ty: RefinementType,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LibraryExport {
    Value(RefinementType),
    Function(Vec<FunctionSignature>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryModule {
    pub specifier: String,
    pub exports: BTreeMap<String, LibraryExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MemberKey {
    receiver: String,
    member: String,
}

impl MemberKey {
    fn new(receiver: &str, member: &str) -> Self {
        Self {
            receiver: receiver.to_string(),
            member: member.to_string(),
        }
    }
}

/// Deterministic ambient type catalog for one resolved runtime environment.
#[derive(Debug, Clone)]
pub struct LibraryRegistry {
    environment: Environment,
    globals: BTreeMap<String, RefinementType>,
    static_functions: BTreeMap<String, Vec<FunctionSignature>>,
    receiver_methods: BTreeMap<MemberKey, Vec<FunctionSignature>>,
    receiver_properties: BTreeMap<MemberKey, PropertySignature>,
    receiver_supertypes: BTreeMap<String, BTreeSet<String>>,
    modules: BTreeMap<String, LibraryModule>,
    module_aliases: BTreeMap<String, String>,
}

impl LibraryRegistry {
    pub(crate) fn empty(environment: Environment) -> Self {
        debug_assert_ne!(environment, Environment::Auto);
        Self {
            environment,
            globals: BTreeMap::new(),
            static_functions: BTreeMap::new(),
            receiver_methods: BTreeMap::new(),
            receiver_properties: BTreeMap::new(),
            receiver_supertypes: BTreeMap::new(),
            modules: BTreeMap::new(),
            module_aliases: BTreeMap::new(),
        }
    }

    pub fn environment(&self) -> Environment {
        self.environment
    }

    pub fn global(&self, name: &str) -> Option<&RefinementType> {
        self.globals.get(name)
    }

    pub fn static_function(&self, path: &str) -> Option<&[FunctionSignature]> {
        self.static_functions.get(path).map(Vec::as_slice)
    }

    pub fn receiver_method(&self, receiver: &str, member: &str) -> Option<&[FunctionSignature]> {
        self.receiver_lineage(receiver)
            .into_iter()
            .find_map(|receiver| {
                self.receiver_methods
                    .get(&MemberKey::new(&receiver, member))
                    .map(Vec::as_slice)
            })
    }

    pub fn receiver_property(&self, receiver: &str, member: &str) -> Option<&PropertySignature> {
        self.receiver_lineage(receiver)
            .into_iter()
            .find_map(|receiver| {
                self.receiver_properties
                    .get(&MemberKey::new(&receiver, member))
            })
    }

    pub fn receiver_is_a(&self, receiver: &str, expected: &str) -> bool {
        self.receiver_lineage(receiver)
            .iter()
            .any(|candidate| candidate == expected)
    }

    pub fn module(&self, specifier: &str) -> Option<&LibraryModule> {
        let canonical = self
            .module_aliases
            .get(specifier)
            .map_or(specifier, String::as_str);
        self.modules.get(canonical)
    }

    pub fn module_export(&self, specifier: &str, export: &str) -> Option<&LibraryExport> {
        self.module(specifier)?.exports.get(export)
    }

    pub fn globals(&self) -> impl Iterator<Item = (&str, &RefinementType)> {
        self.globals.iter().map(|(name, ty)| (name.as_str(), ty))
    }

    pub fn static_functions(&self) -> impl Iterator<Item = (&str, &[FunctionSignature])> {
        self.static_functions
            .iter()
            .map(|(name, overloads)| (name.as_str(), overloads.as_slice()))
    }

    pub fn modules(&self) -> impl Iterator<Item = (&str, &LibraryModule)> {
        self.modules
            .iter()
            .map(|(name, module)| (name.as_str(), module))
    }

    pub fn receiver_methods(&self) -> impl Iterator<Item = (&str, &str, &[FunctionSignature])> {
        self.receiver_methods.iter().map(|(key, overloads)| {
            (
                key.receiver.as_str(),
                key.member.as_str(),
                overloads.as_slice(),
            )
        })
    }

    pub fn receiver_properties(&self) -> impl Iterator<Item = (&str, &str, &PropertySignature)> {
        self.receiver_properties
            .iter()
            .map(|(key, property)| (key.receiver.as_str(), key.member.as_str(), property))
    }

    pub(crate) fn add_global(&mut self, name: &str, ty: RefinementType) {
        if let Some(previous) = self.globals.insert(name.to_string(), ty.clone()) {
            assert_eq!(previous, ty, "conflicting ambient global '{name}'");
        }
    }

    pub(crate) fn add_static_function(&mut self, path: &str, signature: FunctionSignature) {
        self.static_functions
            .entry(path.to_string())
            .or_default()
            .push(signature);
    }

    pub(crate) fn add_receiver_method(
        &mut self,
        receiver: &str,
        member: &str,
        signature: FunctionSignature,
    ) {
        self.receiver_methods
            .entry(MemberKey::new(receiver, member))
            .or_default()
            .push(signature);
    }

    pub(crate) fn add_receiver_property(
        &mut self,
        receiver: &str,
        member: &str,
        property: PropertySignature,
    ) {
        let key = MemberKey::new(receiver, member);
        if let Some(previous) = self.receiver_properties.insert(key, property.clone()) {
            assert_eq!(
                previous, property,
                "conflicting property '{receiver}.{member}'"
            );
        }
    }

    pub(crate) fn add_receiver_supertype(&mut self, receiver: &str, supertype: &str) {
        assert_ne!(receiver, supertype, "a receiver cannot inherit from itself");
        self.receiver_supertypes
            .entry(receiver.to_string())
            .or_default()
            .insert(supertype.to_string());
    }

    pub(crate) fn add_module_export(
        &mut self,
        specifier: &str,
        export: &str,
        value: LibraryExport,
    ) {
        let module = self
            .modules
            .entry(specifier.to_string())
            .or_insert_with(|| LibraryModule {
                specifier: specifier.to_string(),
                exports: BTreeMap::new(),
            });
        match (module.exports.get_mut(export), value) {
            (Some(LibraryExport::Function(overloads)), LibraryExport::Function(mut added)) => {
                overloads.append(&mut added);
            }
            (None, value) => {
                module.exports.insert(export.to_string(), value);
            }
            (Some(previous), value) => {
                assert_eq!(
                    previous, &value,
                    "conflicting export '{specifier}:{export}'"
                );
            }
        }
    }

    pub(crate) fn add_module_alias(&mut self, alias: &str, canonical: &str) {
        assert!(
            self.modules.contains_key(canonical),
            "module alias '{alias}' refers to missing module '{canonical}'"
        );
        if let Some(previous) = self
            .module_aliases
            .insert(alias.to_string(), canonical.to_string())
        {
            assert_eq!(previous, canonical, "conflicting module alias '{alias}'");
        }
    }

    fn receiver_lineage(&self, receiver: &str) -> Vec<String> {
        let mut lineage = Vec::new();
        let mut pending = vec![receiver.to_string()];
        let mut visited = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if let Some(supertypes) = self.receiver_supertypes.get(&current) {
                pending.extend(supertypes.iter().rev().cloned());
            }
            lineage.push(current);
        }
        lineage
    }
}
