## serde-templated-derive

This crate can be used to create serde (de-)serializable types which include fields that are
optionally handlebars templates of a target type.

### Motivation

Consider a configuration format that allows users to insert templated values into the configuration
fields:

```toml
[config]
templated-value = "hello, {{ person }}!"
```

Ultimately you may wish to parse this structure into something like this:

```rust
struct Config {
    templated_value: String,
}
```

A common approach to this is to read the configuration file from disk, render the template, then
parse it as `toml`:

```rust
let config_str = std::fs::read_to_string(cfg_path)?;
let config: Config = toml::from_str(config_str)?;
```

With this approach, the author of the configuration template needs to ensure that the incoming
render context from handlebars cannot accidentally break the TOML formatting that the configuration
adheres to.

For example, if the render context contained something like `\"\ninjected-value = \"mwahahaha!\"`,
it could add unwanted configuration options.

The `Templated` macro allows you to flip the order of "render template -> parse input" to
"parse input -> render templates."
The macro does so by generating a new struct based on the prior struct:


```rust
#[derive(Templated)]
struct Config {
    templated_value: String
}


// generates
struct TemplatedConfig {
    templated_value: serde_templated::Templated<String>
}

impl serde_templated::TemplatedOf for TemplatedConfig {
    type Target = Config;

    /// ...
}
```

This allows you to use serde to parse an input into the `TemplatedConfig`, and then `render` it down
to a `Config` type.


### Default Behavior

By default, the macro generates a struct called `Templated{name}` based on the incoming struct.
Each field will be replaced with `serde_templated::Templated<Type>` where `Type` is the incoming
type.
The exception to this is container types, which will have their interior types replaced instead.
The following types are treated as containers:

* `Option<T>` -> `Option<Templated<T>>`
* `Vec<T>` -> `Vec<Templated<T>>`
* `BTreeMap<K, V>` -> `BTreeMap<Templated<K>, Templated<V>>`
* `BTreeSet<T>` -> `BTreeSet<Templated<T>>`
* `BinaryHeap<T>` -> `BinaryHeap<Templated<T>>`
* `HashMap<K, V>` -> `HashMap<Templated<K>, Templated<V>>`
* `HashSet<T>` -> `HashSet<Templated<T>>`
* `LinkedList<T>` -> `LinkedList<Templated<T>>`
* `VecDeque<T>` -> `VecDeque<Templated<T>>`

The generated `serde_templated::TemplateOf` implementation will treat each field of the generated
struct as having implemented `TemplateOf<Target = T>` where `T` is the type of that same field on
the parent struct.

These behaviors can be overridden with various macro options.

### Macro Options

#### `derive`
By default, the generated `Templated` struct only implements `Serialize` and `Deserialize`.
You can instruct the macro to derive additional traits like so:

```rust
#[derive(Templated)]
#[templated(derive(Default))]
struct MyStruct {
    value: u64,
}
```

#### `forward_attrs`
Allows you to copy an attribute from the "source" struct to the "templated" struct.

```rust
#[derive(Templated)]
#[templated(forward_attrs(serde))]
#[serde(rename_all = "kebab-case")] // The resulting TemplatedMyStruct will also have this attr
struct MyStruct {
    value: u64,

    #[templated(forward_attrs(serde))]  // Can also be specified for fields
    #[serde(rename = "different-name")]
    forwarded_field: String,
}
```

#### `templated_attrs`
This allows you to define arbitrary attributes on the resulting struct, even if they aren't on the
parent struct.

Due to a syntax parsing limitation, these should be given as strings.

```rust
#[derive(Templated)]
#[templated(templated_attrs = "#[derive(Debug)]")]
struct Example {
    #[templated(templated_attrs = "#[serde(alias = \"templated-value\")]")] // Can also be used for fields
    value: Option<u64>,
}
```

#### `skip_serde_derive`
By default, the generated struct will derive implementations of `Serialize` and `Deserialize`.
This behavior can be disabled with the `skip_serde_derive` attribute.

```rust
#[derive(Templated)]
#[templated(skip_serde_derive)]
struct Example {
    // ...
}
```

#### `skip`
Can only be used for fields.

This causes a sub-field of the generated struct to not be "templated" and instead to parse the
given field using the same behavior as the parent struct.

```rust
#[derive(Templated)]
struct Example {
    #[templated(skip)] // `TemplatedExample` will also have this type as a `String`
    value: String,
}
```

#### `template_as`
Can only be used for fields.

Instead of using the default behavior to generate the templated type, fields will be templated
using the provided type.

Due to a limitation of syntax parsing for macros, this is given as a string.

```rust
#[derive(Templated)]
struct Example {
    #[templated(template_as = "Option<Templated<String>>")]
    value: String,
}


// Note, this example will ultimately fail to compile without also adding a `templated(render_with)`
// that converts the optional templated value back into the parent `value` type.
```

#### `render_with`
Can only be used for fields.

By default, the macro assumes that all (non-skipped) fields of the resulting struct will implement
`serde_templated::TemplateOf`, and use that trait to render the field.
You can use an arbitrary function to perform this rendering instead.

```rust
#[derive(Templated)]
struct Example {
    #[templated(render_with = excited)]
    value: String,
}

fn excited(
    to_render: &impl serde_templated::TemplateOf<Target = String>,
    ctx: &impl serde::Serialize,
) -> Result<String, serde_templated::TemplatedError> {
    let res = to_render.render(ctx)?;
    Ok(format!("Wow!!! {res}!!!"))
}
```
