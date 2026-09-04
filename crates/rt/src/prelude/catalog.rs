use crate::syntax::{
    BaseType, BinaryOp, Literal, LogicalOp, PredicateExpr, RefinedParam, RefinementType,
};

use super::{
    CallbackTiming, CallbackUse, Environment, FunctionEffects, FunctionSignature, LibraryExport,
    LibraryParameter, LibraryRegistry, ReceiverEffect, SemanticRefinement,
};

pub(crate) fn build(environment: Environment) -> LibraryRegistry {
    debug_assert_ne!(environment, Environment::Auto);
    let mut registry = LibraryRegistry::empty(environment);
    add_ecmascript(&mut registry);
    match environment {
        Environment::Auto | Environment::Ecmascript => {}
        Environment::Browser => {
            add_web_platform(&mut registry);
            add_dom(&mut registry);
        }
        Environment::Node => {
            add_web_platform(&mut registry);
            add_node(&mut registry);
        }
        Environment::Deno => {
            add_web_platform(&mut registry);
            add_node_modules(&mut registry);
            add_deno(&mut registry);
        }
        Environment::Bun => {
            add_web_platform(&mut registry);
            add_node(&mut registry);
            add_bun(&mut registry);
        }
    }
    registry
}

fn add_ecmascript(registry: &mut LibraryRegistry) {
    for (name, type_name) in [
        ("globalThis", "GlobalThis"),
        ("console", "Console"),
        ("Math", "Math"),
        ("Number", "NumberConstructor"),
        ("String", "StringConstructor"),
        ("Boolean", "BooleanConstructor"),
        ("Object", "ObjectConstructor"),
        ("Array", "ArrayConstructor"),
        ("Promise", "PromiseConstructor"),
        ("JSON", "JSON"),
    ] {
        registry.add_global(name, named_type(type_name));
    }
    registry.add_global("undefined", primitive_type("undefined"));

    for method in ["log", "info", "warn", "error", "debug"] {
        registry.add_receiver_method(
            "Console",
            method,
            FunctionSignature::new(
                vec![LibraryParameter::rest("values", primitive_type("unknown"))],
                primitive_type("void"),
            )
            .with_receiver(named_type("Console")),
        );
    }

    let x_non_nan = RefinementType {
        base: primitive("number"),
        index: None,
        predicate: Some(PredicateExpr::Binary(
            BinaryOp::EqEqEq,
            Box::new(PredicateExpr::Identifier("x".into())),
            Box::new(PredicateExpr::Identifier("x".into())),
        )),
    };
    registry.add_static_function(
        "Math.sqrt",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "x",
                number_parameter_at_least("x", 0),
            )],
            sqrt_result(),
        ),
    );
    registry.add_static_function(
        "Math.abs",
        FunctionSignature::new(
            vec![LibraryParameter::required("x", x_non_nan)],
            non_negative_number(),
        ),
    );
    for name in ["Math.floor", "Math.ceil", "Math.round", "Math.trunc"] {
        registry.add_static_function(
            name,
            FunctionSignature::new(
                vec![LibraryParameter::required("x", primitive_type("number"))],
                primitive_type("number"),
            ),
        );
    }
    for name in ["Math.min", "Math.max"] {
        registry.add_static_function(
            name,
            FunctionSignature::new(
                vec![LibraryParameter::rest("values", primitive_type("number"))],
                primitive_type("number"),
            ),
        );
    }

    registry.add_static_function(
        "Array.isArray",
        FunctionSignature::new(
            vec![LibraryParameter::required("x", primitive_type("unknown"))],
            primitive_type("boolean"),
        ),
    );
    registry.add_static_function(
        "Array.from",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "items",
                generic_type("Iterable", vec![type_variable("$T")]),
            )],
            array_type(type_variable("$T")),
        ),
    );
    registry.add_static_function(
        "Number.isInteger",
        FunctionSignature::new(
            vec![LibraryParameter::required("x", primitive_type("unknown"))],
            primitive_type("boolean"),
        ),
    );
    registry.add_static_function(
        "Number.isFinite",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "value",
                primitive_type("unknown"),
            )],
            primitive_type("boolean"),
        ),
    );
    registry.add_static_function(
        "parseInt",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("text", primitive_type("string")),
                LibraryParameter::optional("radix", primitive_type("number")),
            ],
            primitive_type("number"),
        ),
    );

    registry.add_receiver_property(
        "Array",
        "length",
        non_negative_number(),
    );
    registry.add_receiver_property(
        "String",
        "length",
        non_negative_number(),
    );
    add_array_methods(registry);
}

fn add_array_methods(registry: &mut LibraryRegistry) {
    let receiver = array_type(type_variable("$T"));
    let element_callback = callback_type(
        vec![
            ("value", type_variable("$T")),
            ("index", primitive("number")),
            ("array", BaseType::Array(Box::new(type_variable("$T")))),
        ],
        type_variable("$U"),
    );
    let flat_map_array_callback = callback_type(
        vec![
            ("value", type_variable("$T")),
            ("index", primitive("number")),
            ("array", BaseType::Array(Box::new(type_variable("$T")))),
        ],
        BaseType::Array(Box::new(type_variable("$U"))),
    );
    for callback in [flat_map_array_callback, element_callback.clone()] {
        registry.add_receiver_method(
            "Array",
            "flatMap",
            FunctionSignature::new(
                vec![LibraryParameter::required("callback", callback)],
                array_type(type_variable("$U")),
            )
            .with_receiver(receiver.clone())
            .with_effects(callback_effects(0, CallbackTiming::Immediate)),
        );
    }
    registry.add_receiver_method(
        "Array",
        "map",
        FunctionSignature::new(
            vec![LibraryParameter::required("callback", element_callback)],
            array_type(type_variable("$U")),
        )
        .with_receiver(receiver.clone())
        .with_effects(callback_effects(0, CallbackTiming::Immediate))
        .with_refinements(vec![SemanticRefinement::ResultLengthEqualsReceiver]),
    );

    let predicate_callback = callback_type(
        vec![
            ("value", type_variable("$T")),
            ("index", primitive("number")),
            ("array", BaseType::Array(Box::new(type_variable("$T")))),
        ],
        primitive("boolean"),
    );
    registry.add_receiver_method(
        "Array",
        "filter",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "predicate",
                predicate_callback.clone(),
            )],
            array_type(type_variable("$T")),
        )
        .with_receiver(receiver.clone())
        .with_effects(callback_effects(0, CallbackTiming::Immediate))
        .with_refinements(vec![SemanticRefinement::ResultLengthAtMostReceiver]),
    );
    for method in ["every", "some"] {
        registry.add_receiver_method(
            "Array",
            method,
            FunctionSignature::new(
                vec![LibraryParameter::required(
                    "predicate",
                    predicate_callback.clone(),
                )],
                primitive_type("boolean"),
            )
            .with_receiver(receiver.clone())
            .with_effects(callback_effects(0, CallbackTiming::Immediate)),
        );
    }
    registry.add_receiver_method(
        "Array",
        "forEach",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "callback",
                callback_type(
                    vec![
                        ("value", type_variable("$T")),
                        ("index", primitive("number")),
                        ("array", BaseType::Array(Box::new(type_variable("$T")))),
                    ],
                    primitive("void"),
                ),
            )],
            primitive_type("void"),
        )
        .with_receiver(receiver.clone())
        .with_effects(callback_effects(0, CallbackTiming::Immediate)),
    );
    registry.add_receiver_method(
        "Array",
        "find",
        FunctionSignature::new(
            vec![LibraryParameter::required("predicate", predicate_callback)],
            optional_type(type_variable("$T")),
        )
        .with_receiver(receiver.clone())
        .with_effects(callback_effects(0, CallbackTiming::Immediate)),
    );

    let reduce_callback = callback_type(
        vec![
            ("accumulator", type_variable("$U")),
            ("value", type_variable("$T")),
            ("index", primitive("number")),
            ("array", BaseType::Array(Box::new(type_variable("$T")))),
        ],
        type_variable("$U"),
    );
    registry.add_receiver_method(
        "Array",
        "reduce",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "callback",
                callback_type(
                    vec![
                        ("accumulator", type_variable("$T")),
                        ("value", type_variable("$T")),
                        ("index", primitive("number")),
                        ("array", BaseType::Array(Box::new(type_variable("$T")))),
                    ],
                    type_variable("$T"),
                ),
            )],
            type_variable_type("$T"),
        )
        .with_receiver(receiver.clone())
        .with_effects(callback_effects(0, CallbackTiming::Immediate)),
    );
    registry.add_receiver_method(
        "Array",
        "reduce",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("callback", reduce_callback),
                LibraryParameter::required("initialValue", type_variable_type("$U")),
            ],
            type_variable_type("$U"),
        )
        .with_receiver(receiver.clone())
        .with_effects(callback_effects(0, CallbackTiming::Immediate)),
    );

    registry.add_receiver_method(
        "Array",
        "slice",
        FunctionSignature::new(
            vec![
                LibraryParameter::optional("start", primitive_type("number")),
                LibraryParameter::optional("end", primitive_type("number")),
            ],
            array_type(type_variable("$T")),
        )
        .with_receiver(receiver.clone())
        .with_refinements(vec![SemanticRefinement::ResultLengthAtMostReceiver]),
    );
    registry.add_receiver_method(
        "Array",
        "includes",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("value", type_variable_type("$T")),
                LibraryParameter::optional("fromIndex", primitive_type("number")),
            ],
            primitive_type("boolean"),
        )
        .with_receiver(receiver.clone()),
    );
    registry.add_receiver_method(
        "Array",
        "at",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "index",
                primitive_type("number"),
            )],
            optional_type(type_variable("$T")),
        )
        .with_receiver(receiver.clone()),
    );
    registry.add_receiver_method(
        "Array",
        "push",
        FunctionSignature::new(
            vec![LibraryParameter::rest("items", type_variable_type("$T"))],
            non_negative_number(),
        )
        .with_receiver(receiver.clone())
        .with_effects(receiver_effects(ReceiverEffect::Mutate))
        .with_refinements(vec![
            SemanticRefinement::ReceiverLengthIncreasesByArgumentCount,
            SemanticRefinement::ReceiverMayContainArguments,
        ]),
    );
    registry.add_receiver_method(
        "Array",
        "pop",
        FunctionSignature::new(Vec::new(), optional_type(type_variable("$T")))
            .with_receiver(receiver)
            .with_effects(receiver_effects(ReceiverEffect::Mutate)),
    );
    registry.add_receiver_method(
        "DenseArray",
        "pop",
        FunctionSignature::new(Vec::new(), type_variable_type("$T"))
            .with_receiver(generic_type("DenseArray", vec![type_variable("$T")]))
            .with_effects(receiver_effects(ReceiverEffect::Mutate))
            .with_refinements(vec![
                SemanticRefinement::RequiresPositiveReceiverLength,
                SemanticRefinement::ReceiverLengthDecreasesByOne,
            ]),
    );
    registry.add_receiver_method(
        "DenseArray",
        "push",
        FunctionSignature::new(
            vec![LibraryParameter::rest("items", type_variable_type("$T"))],
            non_negative_number(),
        )
        .with_receiver(generic_type("DenseArray", vec![type_variable("$T")]))
        .with_effects(receiver_effects(ReceiverEffect::Mutate))
        .with_refinements(vec![
            SemanticRefinement::ReceiverLengthIncreasesByArgumentCount,
            SemanticRefinement::ReceiverMayContainArguments,
        ]),
    );
}

fn add_web_platform(registry: &mut LibraryRegistry) {
    for (name, type_name) in [
        ("Request", "RequestConstructor"),
        ("Response", "ResponseConstructor"),
        ("Headers", "HeadersConstructor"),
        ("URL", "URLConstructor"),
        ("URLSearchParams", "URLSearchParamsConstructor"),
        ("AbortController", "AbortControllerConstructor"),
        ("TextEncoder", "TextEncoderConstructor"),
        ("TextDecoder", "TextDecoderConstructor"),
        ("Blob", "BlobConstructor"),
        ("FormData", "FormDataConstructor"),
    ] {
        registry.add_global(name, named_type(type_name));
    }
    registry.add_static_function(
        "fetch",
        FunctionSignature::new(
            vec![
                LibraryParameter::required(
                    "input",
                    union_type(vec![primitive("string"), named("Request"), named("URL")]),
                ),
                LibraryParameter::optional("init", primitive_type("object")),
            ],
            promise_type(named("Response")),
        ),
    );
    registry.add_static_function(
        "structuredClone",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "value",
                type_variable_type("$T"),
            )],
            type_variable_type("$T"),
        ),
    );
    registry.add_receiver_property(
        "Response",
        "status",
        non_negative_number(),
    );
    registry.add_receiver_property(
        "Response",
        "ok",
        primitive_type("boolean"),
    );
    registry.add_receiver_method(
        "Response",
        "text",
        FunctionSignature::new(Vec::new(), promise_type(primitive("string")))
            .with_receiver(named_type("Response")),
    );
    registry.add_receiver_method(
        "Response",
        "json",
        FunctionSignature::new(Vec::new(), promise_type(primitive("unknown")))
            .with_receiver(named_type("Response")),
    );
    registry.add_receiver_method(
        "Headers",
        "get",
        FunctionSignature::new(
            vec![LibraryParameter::required("name", primitive_type("string"))],
            nullable_type(primitive("string")),
        )
        .with_receiver(named_type("Headers")),
    );
}

fn add_dom(registry: &mut LibraryRegistry) {
    for (receiver, supertype) in [
        ("Node", "EventTarget"),
        ("Document", "Node"),
        ("DocumentFragment", "Node"),
        ("Element", "Node"),
        ("HTMLElement", "Element"),
        ("HTMLButtonElement", "HTMLElement"),
        ("Window", "EventTarget"),
    ] {
        registry.add_receiver_supertype(receiver, supertype);
    }
    for (name, type_name) in [
        ("window", "Window"),
        ("document", "Document"),
        ("navigator", "Navigator"),
        ("customElements", "CustomElementRegistry"),
        ("Node", "NodeConstructor"),
        ("Element", "ElementConstructor"),
        ("HTMLElement", "HTMLElementConstructor"),
        ("Event", "EventConstructor"),
    ] {
        registry.add_global(name, named_type(type_name));
    }
    registry.add_static_function(
        "setTimeout",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("handler", callback_type(Vec::new(), primitive("void"))),
                LibraryParameter::optional("timeout", primitive_type("number")),
                LibraryParameter::rest("arguments", primitive_type("unknown")),
            ],
            non_negative_number(),
        )
        .with_effects(callback_effects(0, CallbackTiming::Deferred)),
    );
    registry.add_static_function(
        "clearTimeout",
        FunctionSignature::new(
            vec![LibraryParameter::required("id", primitive_type("number"))],
            primitive_type("void"),
        ),
    );

    for receiver in ["NodeList", "HTMLCollection"] {
        registry.add_receiver_property(
            receiver,
            "length",
            non_negative_number(),
        );
    }
    registry.add_receiver_property(
        "Element",
        "childElementCount",
        non_negative_number(),
    );
    registry.add_receiver_property(
        "Element",
        "children",
        named_type("HTMLCollection"),
    );
    registry.add_receiver_property(
        "Node",
        "textContent",
        nullable_type(primitive("string")),
    );
    registry.add_receiver_method(
        "Node",
        "appendChild",
        FunctionSignature::new(
            vec![LibraryParameter::required("node", named_type("Node"))],
            named_type("Node"),
        )
        .with_receiver(named_type("Node"))
        .with_effects(user_code_effects(ReceiverEffect::Mutate)),
    );
    registry.add_receiver_method(
        "HTMLElement",
        "click",
        FunctionSignature::new(Vec::new(), primitive_type("void"))
            .with_receiver(named_type("HTMLElement"))
            .with_effects(user_code_effects(ReceiverEffect::Mutate)),
    );
    registry.add_receiver_property(
        "Document",
        "body",
        nullable_type(named("HTMLElement")),
    );

    registry.add_receiver_method(
        "Document",
        "querySelector",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "selectors",
                primitive_type("string"),
            )],
            nullable_type(named("Element")),
        )
        .with_receiver(named_type("Document")),
    );
    registry.add_receiver_method(
        "Document",
        "querySelectorAll",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "selectors",
                primitive_type("string"),
            )],
            named_type("NodeList"),
        )
        .with_receiver(named_type("Document")),
    );
    registry.add_receiver_method(
        "Document",
        "getElementById",
        FunctionSignature::new(
            vec![LibraryParameter::required("id", primitive_type("string"))],
            nullable_type(named("HTMLElement")),
        )
        .with_receiver(named_type("Document")),
    );
    registry.add_receiver_method(
        "Document",
        "createElement",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "localName",
                primitive_type("string"),
            )],
            named_type("HTMLElement"),
        )
        .with_receiver(named_type("Document")),
    );
    registry.add_receiver_method(
        "EventTarget",
        "addEventListener",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("type", primitive_type("string")),
                LibraryParameter::required(
                    "listener",
                    callback_type(
                        vec![("event", BaseType::Named("Event".into()))],
                        primitive("void"),
                    ),
                ),
                LibraryParameter::optional("options", named_type("EventListenerOptions")),
            ],
            primitive_type("void"),
        )
        .with_receiver(named_type("EventTarget"))
        .with_effects(callback_effects(1, CallbackTiming::Deferred)),
    );
    registry.add_receiver_method(
        "EventTarget",
        "dispatchEvent",
        FunctionSignature::new(
            vec![LibraryParameter::required("event", named_type("Event"))],
            primitive_type("boolean"),
        )
        .with_receiver(named_type("EventTarget"))
        .with_effects(user_code_effects(ReceiverEffect::None)),
    );
}

fn add_node(registry: &mut LibraryRegistry) {
    add_node_globals(registry);
    add_node_modules(registry);
}

fn add_node_globals(registry: &mut LibraryRegistry) {
    for (name, type_name) in [
        ("process", "NodeJS.Process"),
        ("Buffer", "BufferConstructor"),
        ("global", "NodeJS.Global"),
    ] {
        registry.add_global(name, named_type(type_name));
    }
    registry.add_global("__dirname", primitive_type("string"));
    registry.add_global("__filename", primitive_type("string"));
    registry.add_global("require", named_type("NodeRequire"));
    registry.add_receiver_supertype("Buffer", "Uint8Array");
    for receiver in ["Buffer", "Uint8Array"] {
        registry.add_receiver_property(
            receiver,
            "length",
            non_negative_number(),
        );
    }
    registry.add_receiver_property(
        "NodeJS.Process",
        "argv",
        array_type(primitive("string")),
    );

    registry.add_static_function(
        "process.cwd",
        FunctionSignature::new(Vec::new(), primitive_type("string")),
    );
    registry.add_static_function(
        "process.exit",
        FunctionSignature::new(
            vec![LibraryParameter::optional("code", primitive_type("number"))],
            primitive_type("never"),
        ),
    );
    registry.add_static_function(
        "Buffer.byteLength",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("value", primitive_type("string")),
                LibraryParameter::optional("encoding", primitive_type("string")),
            ],
            non_negative_number(),
        ),
    );
    registry.add_static_function(
        "Buffer.isBuffer",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "value",
                primitive_type("unknown"),
            )],
            primitive_type("boolean"),
        ),
    );
    registry.add_static_function(
        "setTimeout",
        FunctionSignature::new(
            vec![
                LibraryParameter::required(
                    "callback",
                    callback_type(Vec::new(), primitive("void")),
                ),
                LibraryParameter::optional("delay", primitive_type("number")),
                LibraryParameter::rest("arguments", primitive_type("unknown")),
            ],
            named_type("NodeJS.Timeout"),
        )
        .with_effects(callback_effects(0, CallbackTiming::Deferred)),
    );
    registry.add_static_function(
        "clearTimeout",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "timeout",
                union_type(vec![primitive("number"), named("NodeJS.Timeout")]),
            )],
            primitive_type("void"),
        ),
    );
}

fn add_node_modules(registry: &mut LibraryRegistry) {
    add_node_fs(registry);
    add_node_path(registry);
    registry.add_module_export(
        "node:os",
        "homedir",
        function_export(FunctionSignature::new(Vec::new(), primitive_type("string"))),
    );
    registry.add_module_export(
        "node:events",
        "EventEmitter",
        LibraryExport::Value(named_type("EventEmitterConstructor")),
    );
    registry.add_module_alias("os", "node:os");
    registry.add_module_alias("events", "node:events");
}

fn add_node_fs(registry: &mut LibraryRegistry) {
    registry.add_module_export(
        "node:fs",
        "readFileSync",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::required("path", node_path_type())],
            named_type("Buffer"),
        )),
    );
    registry.add_module_export(
        "node:fs",
        "readFileSync",
        function_export(FunctionSignature::new(
            vec![
                LibraryParameter::required("path", node_path_type()),
                LibraryParameter::required("encoding", primitive_type("string")),
            ],
            primitive_type("string"),
        )),
    );
    registry.add_module_export(
        "node:fs",
        "existsSync",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::required("path", node_path_type())],
            primitive_type("boolean"),
        )),
    );
    registry.add_module_export(
        "node:fs",
        "writeFileSync",
        function_export(
            FunctionSignature::new(
                vec![
                    LibraryParameter::required("path", node_path_type()),
                    LibraryParameter::required(
                        "data",
                        union_type(vec![primitive("string"), named("Buffer")]),
                    ),
                ],
                primitive_type("void"),
            ),
        ),
    );
    registry.add_module_alias("fs", "node:fs");

    registry.add_module_export(
        "node:fs/promises",
        "readFile",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::required("path", node_path_type())],
            promise_type(named("Buffer")),
        )),
    );
    registry.add_module_export(
        "node:fs/promises",
        "readFile",
        function_export(FunctionSignature::new(
            vec![
                LibraryParameter::required("path", node_path_type()),
                LibraryParameter::required("encoding", primitive_type("string")),
            ],
            promise_type(primitive("string")),
        )),
    );
    registry.add_module_export(
        "node:fs/promises",
        "writeFile",
        function_export(
            FunctionSignature::new(
                vec![
                    LibraryParameter::required("path", node_path_type()),
                    LibraryParameter::required(
                        "data",
                        union_type(vec![primitive("string"), named("Buffer")]),
                    ),
                ],
                promise_type(primitive("void")),
            ),
        ),
    );
    registry.add_module_alias("fs/promises", "node:fs/promises");
}

fn add_node_path(registry: &mut LibraryRegistry) {
    registry.add_module_export(
        "node:path",
        "join",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::rest("paths", primitive_type("string"))],
            primitive_type("string"),
        )),
    );
    for export in ["basename", "dirname", "extname", "normalize"] {
        registry.add_module_export(
            "node:path",
            export,
            function_export(FunctionSignature::new(
                vec![LibraryParameter::required("path", primitive_type("string"))],
                primitive_type("string"),
            )),
        );
    }
    registry.add_module_export(
        "node:path",
        "resolve",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::rest("paths", primitive_type("string"))],
            primitive_type("string"),
        )),
    );
    registry.add_module_export(
        "node:path",
        "sep",
        LibraryExport::Value(primitive_type("string")),
    );
    registry.add_module_alias("path", "node:path");
}

fn add_deno(registry: &mut LibraryRegistry) {
    add_node_globals(registry);
    registry.add_global("Deno", named_type("Deno.Namespace"));
    registry.add_static_function(
        "Deno.cwd",
        FunctionSignature::new(Vec::new(), primitive_type("string")),
    );
    registry.add_static_function(
        "Deno.readTextFile",
        FunctionSignature::new(
            vec![LibraryParameter::required("path", deno_path_type())],
            promise_type(primitive("string")),
        ),
    );
    registry.add_static_function(
        "Deno.readFile",
        FunctionSignature::new(
            vec![LibraryParameter::required("path", deno_path_type())],
            promise_type(named("Uint8Array")),
        ),
    );
    registry.add_static_function(
        "Deno.writeTextFile",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("path", deno_path_type()),
                LibraryParameter::required("data", primitive_type("string")),
            ],
            promise_type(primitive("void")),
        ),
    );
    registry.add_static_function(
        "Deno.exit",
        FunctionSignature::new(
            vec![LibraryParameter::optional("code", primitive_type("number"))],
            primitive_type("never"),
        ),
    );
    registry.add_receiver_property(
        "Deno.Namespace",
        "args",
        array_type(primitive("string")),
    );

    registry.add_module_export(
        "jsr:@std/path",
        "join",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::rest("paths", primitive_type("string"))],
            primitive_type("string"),
        )),
    );
    registry.add_module_export(
        "jsr:@std/assert",
        "assert",
        function_export(FunctionSignature::new(
            vec![
                LibraryParameter::required("expression", primitive_type("boolean")),
                LibraryParameter::optional("message", primitive_type("string")),
            ],
            primitive_type("void"),
        )),
    );
}

fn add_bun(registry: &mut LibraryRegistry) {
    registry.add_global("Bun", named_type("Bun.Namespace"));
    registry.add_receiver_property(
        "Bun.Namespace",
        "version",
        primitive_type("string"),
    );
    registry.add_receiver_property(
        "Bun.Namespace",
        "argv",
        array_type(primitive("string")),
    );
    registry.add_static_function(
        "Bun.stringWidth",
        FunctionSignature::new(
            vec![LibraryParameter::required("text", primitive_type("string"))],
            non_negative_number(),
        ),
    );
    registry.add_static_function(
        "Bun.file",
        FunctionSignature::new(
            vec![LibraryParameter::required("path", bun_path_type())],
            named_type("BunFile"),
        ),
    );
    registry.add_static_function(
        "Bun.write",
        FunctionSignature::new(
            vec![
                LibraryParameter::required("destination", bun_path_type()),
                LibraryParameter::required(
                    "input",
                    union_type(vec![primitive("string"), named("Blob")]),
                ),
            ],
            promise_type(primitive("number")),
        ),
    );
    registry.add_static_function(
        "Bun.sleep",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "milliseconds",
                primitive_type("number"),
            )],
            promise_type(primitive("void")),
        ),
    );
    registry.add_static_function(
        "Bun.nanoseconds",
        FunctionSignature::new(Vec::new(), non_negative_number()),
    );
    registry.add_static_function(
        "Bun.serve",
        FunctionSignature::new(
            vec![LibraryParameter::required(
                "options",
                primitive_type("object"),
            )],
            named_type("Bun.Server"),
        )
        .with_effects(FunctionEffects {
            executes_user_code: true,
            ..FunctionEffects::default()
        }),
    );
    registry.add_receiver_method(
        "BunFile",
        "text",
        FunctionSignature::new(Vec::new(), promise_type(primitive("string")))
            .with_receiver(named_type("BunFile")),
    );
    registry.add_receiver_property(
        "BunFile",
        "size",
        non_negative_number(),
    );
    registry.add_receiver_property(
        "Bun.Server",
        "port",
        non_negative_number(),
    );
    registry.add_receiver_method(
        "Bun.Server",
        "stop",
        FunctionSignature::new(
            vec![LibraryParameter::optional(
                "closeActiveConnections",
                primitive_type("boolean"),
            )],
            primitive_type("void"),
        )
        .with_receiver(named_type("Bun.Server"))
        .with_effects(FunctionEffects {
            invalidates_heap_facts: true,
            ..FunctionEffects::default()
        }),
    );

    registry.add_module_export(
        "bun:test",
        "test",
        function_export(
            FunctionSignature::new(
                vec![
                    LibraryParameter::required("name", primitive_type("string")),
                    LibraryParameter::required(
                        "body",
                        callback_type(
                            Vec::new(),
                            BaseType::Union(vec![
                                primitive("void"),
                                BaseType::Generic("Promise".into(), vec![primitive("void")]),
                            ]),
                        ),
                    ),
                ],
                primitive_type("void"),
            )
            .with_effects(callback_effects(1, CallbackTiming::Deferred)),
        ),
    );
    registry.add_module_export(
        "bun:test",
        "expect",
        function_export(FunctionSignature::new(
            vec![LibraryParameter::required(
                "actual",
                type_variable_type("$T"),
            )],
            named_type("Bun.Matchers"),
        )),
    );
}

fn primitive(name: &str) -> BaseType {
    BaseType::Primitive(name.to_string())
}

fn named(name: &str) -> BaseType {
    BaseType::Named(name.to_string())
}

fn type_variable(name: &str) -> BaseType {
    assert!(name.starts_with('$'));
    named(name)
}

fn primitive_type(name: &str) -> RefinementType {
    RefinementType {
        base: primitive(name),
        index: None,
        predicate: None,
    }
}

fn named_type(name: &str) -> RefinementType {
    RefinementType {
        base: named(name),
        index: None,
        predicate: None,
    }
}

fn type_variable_type(name: &str) -> RefinementType {
    RefinementType {
        base: type_variable(name),
        index: None,
        predicate: None,
    }
}

fn array_type(element: BaseType) -> RefinementType {
    RefinementType {
        base: BaseType::Array(Box::new(element)),
        index: None,
        predicate: None,
    }
}

fn generic_type(name: &str, arguments: Vec<BaseType>) -> RefinementType {
    RefinementType {
        base: BaseType::Generic(name.to_string(), arguments),
        index: None,
        predicate: None,
    }
}

fn union_type(members: Vec<BaseType>) -> RefinementType {
    RefinementType {
        base: BaseType::Union(members),
        index: None,
        predicate: None,
    }
}

fn promise_type(value: BaseType) -> RefinementType {
    generic_type("Promise", vec![value])
}

fn nullable_type(value: BaseType) -> RefinementType {
    union_type(vec![value, primitive("null")])
}

fn optional_type(value: BaseType) -> RefinementType {
    union_type(vec![value, primitive("undefined")])
}

fn node_path_type() -> RefinementType {
    union_type(vec![primitive("string"), named("Buffer"), named("URL")])
}

fn deno_path_type() -> RefinementType {
    union_type(vec![primitive("string"), named("URL")])
}

fn bun_path_type() -> RefinementType {
    union_type(vec![primitive("string"), named("URL"), named("Uint8Array")])
}

fn callback_type(params: Vec<(&str, BaseType)>, returns: BaseType) -> RefinementType {
    RefinementType {
        base: BaseType::Function(
            params
                .into_iter()
                .map(|(name, base)| RefinedParam {
                    name: name.to_string(),
                    ty: RefinementType::from_base(base),
                })
                .collect(),
            Box::new(RefinementType::from_base(returns)),
        ),
        index: None,
        predicate: None,
    }
}

fn number_parameter_at_least(name: &str, minimum: i64) -> RefinementType {
    RefinementType {
        base: primitive("number"),
        index: None,
        predicate: Some(PredicateExpr::Binary(
            BinaryOp::Gte,
            Box::new(PredicateExpr::Identifier(name.to_string())),
            Box::new(PredicateExpr::Literal(Literal::Number(minimum as f64))),
        )),
    }
}

fn non_negative_number() -> RefinementType {
    RefinementType {
        base: primitive("number"),
        index: None,
        predicate: Some(PredicateExpr::Binary(
            BinaryOp::Gte,
            Box::new(PredicateExpr::Return),
            Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
        )),
    }
}

fn sqrt_result() -> RefinementType {
    RefinementType {
        base: primitive("number"),
        index: None,
        predicate: Some(PredicateExpr::Logical(
            LogicalOp::And,
            Box::new(PredicateExpr::Binary(
                BinaryOp::Gte,
                Box::new(PredicateExpr::Return),
                Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
            )),
            Box::new(PredicateExpr::Logical(
                LogicalOp::Or,
                Box::new(PredicateExpr::Binary(
                    BinaryOp::EqEqEq,
                    Box::new(PredicateExpr::Identifier("x".into())),
                    Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                )),
                Box::new(PredicateExpr::Binary(
                    BinaryOp::Gt,
                    Box::new(PredicateExpr::Return),
                    Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                )),
            )),
        )),
    }
}

fn receiver_effects(receiver: ReceiverEffect) -> FunctionEffects {
    FunctionEffects {
        receiver,
        ..FunctionEffects::default()
    }
}

fn user_code_effects(receiver: ReceiverEffect) -> FunctionEffects {
    FunctionEffects {
        receiver,
        executes_user_code: true,
        ..FunctionEffects::default()
    }
}

fn callback_effects(parameter_index: usize, timing: CallbackTiming) -> FunctionEffects {
    FunctionEffects {
        receiver: ReceiverEffect::None,
        callbacks: vec![CallbackUse {
            parameter_index,
            timing,
        }],
        ..FunctionEffects::default()
    }
}

fn function_export(signature: FunctionSignature) -> LibraryExport {
    LibraryExport::Function(vec![signature])
}
