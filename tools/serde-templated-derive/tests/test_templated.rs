use serde_templated::{TemplateOf, Templated};
use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;

#[derive(Templated, Default)]
#[templated(derive(Default))]
#[allow(
    dead_code,
    reason = "Tests are done on macro-generated struct, not this one"
)]
struct MyConfig {
    field_a: String,
    field_b: Option<u64>,
    #[templated(skip)]
    field_c: i32,
}

#[test]
fn basic_field_wrapping() {
    let _templated_config = TemplatedMyConfig {
        field_a: Templated::raw("Hello".to_string()),
        ..Default::default()
    };
}

#[test]
fn inner_optional_wrapping() {
    let _templated_config = TemplatedMyConfig {
        field_b: Some(Templated::raw(100u64)),
        ..Default::default()
    };
}

#[test]
fn skipped_values() {
    let _templated_config = TemplatedMyConfig {
        field_c: 25i32,
        ..Default::default()
    };
}

#[derive(Templated, Default)]
#[templated(skip_serde_derive, templated_attrs = ["#[derive(Debug)]"], derive(Default))]
#[allow(
    dead_code,
    reason = "Tests are done on macro-generated struct, not this one"
)]
struct GenericConfig<T>
where
    <T as FromStr>::Err: std::error::Error + Send + Sync + 'static,
    T: FromStr + Default + Clone + Debug,
{
    field_a: T,
}

#[test]
fn test_generics_respected() {
    let _templated_config: TemplatedGenericConfig<u64> = TemplatedGenericConfig {
        field_a: Templated::raw(100u64),
    };
}

#[derive(Templated)]
#[allow(
    dead_code,
    reason = "Tests are done on macro-generated struct, not this one"
)]
struct HashMapConfig {
    field_a: HashMap<String, String>,
}

#[test]
fn test_templated_hashmap() {
    let mut map = HashMap::new();
    map.insert(
        Templated::template("{{ hello }}"),
        Templated::raw("world".to_string()),
    );

    let _templated_config = TemplatedHashMapConfig { field_a: map };
}

#[derive(Debug, Templated, PartialEq, Eq)]
struct FriendlyConfig {
    #[templated(render_with = friendly_render)]
    field: String,
}

fn friendly_render(
    to_render: &impl serde_templated::TemplateOf<Target = String>,
    ctx: &impl serde::Serialize,
) -> Result<String, serde_templated::TemplatedError> {
    let res = to_render.render(ctx)?;
    Ok(format!("Hey, I got this for you: {res}"))
}

#[test]
fn test_friendly_render() {
    let templated_config = TemplatedFriendlyConfig {
        field: Templated::raw("🍰".to_string()),
    };

    let rendered = templated_config.render(&()).unwrap();
    assert_eq!(
        rendered,
        FriendlyConfig {
            field: "Hey, I got this for you: 🍰".to_string()
        }
    );
}

/// Test that serde attributes are forwarded to the inner struct
///
/// Defined in a module to avoid adding `Serialize` and `Deserialize` to the
/// test namespace.
pub(crate) mod serde_fwd {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Templated, Serialize, Deserialize)]
    #[templated(forward_attrs(serde))]
    #[serde(rename_all = "kebab-case")]
    pub(crate) struct ForwardsSerdeAttributes {
        #[templated(forward_attrs(serde))]
        #[serde(rename = "renamed")]
        pub name: String,
        pub kebab_case: String,
    }
}

#[test]
fn test_serde_fwd() {
    use serde_fwd::*;

    let parsed: TemplatedForwardsSerdeAttributes = serde_json::from_value(serde_json::json!({
        "renamed": "value",
        "kebab-case": "kebab"
    }))
    .unwrap();
    assert_eq!(parsed.render(&()).unwrap().name, "value");
    assert_eq!(parsed.render(&()).unwrap().kebab_case, "kebab");
}
