//! Provides a serde-parseable "template" type, [`Templated`], which can be parsed as either
//! directly as the final desired type, or as a handlebars template which will eventually parse to
//! the desired type.
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::fmt::Debug;

mod template_of;
mod templated;

pub use serde_templated_derive::Templated;
pub use templated::Templated;

/// Represents a type that can be rendered to another, interpolating values from some context.
///
/// This is generally implemented using the [`Template`] struct under the hood, which uses
/// [`handlebars`] as its underlying templating engine.
pub trait TemplateOf {
    type Target;

    fn render(&self, template_context: &impl Serialize) -> Result<Self::Target, TemplatedError>;

    /// Same behavior as `render`, but embeds information about the current field in error messages.
    fn render_field(
        &self,
        field: &str,
        template_context: &impl Serialize,
    ) -> Result<Self::Target, TemplatedError> {
        use templated_error::*;

        self.render(template_context)
            .context(RenderFieldSnafu { field })
    }
}

/// A string that can be rendered as a template to another string.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Default, Ord, PartialOrd, Hash)]
#[serde(transparent)]
pub struct Template(String);

impl Template {
    pub fn new(template_str: impl Into<String>) -> Self {
        Self(template_str.into())
    }
}

impl TemplateOf for Template {
    type Target = String;

    fn render(&self, template_context: &impl Serialize) -> Result<Self::Target, TemplatedError> {
        use templated_error::*;

        let mut registry = handlebars::Handlebars::new();
        registry.set_strict_mode(true);
        registry
            .render_template(self.0.as_ref(), template_context)
            .context(RenderSnafu)
    }
}

impl<S: Into<String>> From<S> for Template {
    fn from(value: S) -> Self {
        Self::new(value.into())
    }
}

impl AsRef<str> for Template {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

#[derive(Debug, Snafu)]
#[snafu(module, visibility(pub(crate)))]
pub enum TemplatedError {
    #[snafu(display("Template rendered to invalid value '{value}': {source}"))]
    InvalidValue {
        source: Box<dyn std::error::Error + Send + Sync>,
        value: String,
    },

    #[snafu(display(
        "Rendering Dictionary type resulted in collision for fields rendered to '{render_result}'"
    ))]
    RenderCollision { render_result: String },

    #[snafu(display("Failed to render field '{field}': {source}"))]
    RenderField {
        #[snafu(source(from(TemplatedError, Box::new)))]
        source: Box<TemplatedError>,
        field: String,
    },

    #[snafu(display("Failed to render template: {source}"))]
    RenderError { source: handlebars::RenderError },
}

#[cfg(test)]
mod test {
    use super::*;
    use bounded_integer::BoundedU64;
    use maplit::btreemap;
    use serde_json::json;

    #[test]
    fn test_maybe_templated_parse_u32() {
        let maybe_templated: Templated<u32> = serde_json::from_value(json!(1)).unwrap();
        assert_eq!(maybe_templated.render(&()).unwrap(), 1);
    }

    #[test]
    fn test_maybe_templated_parse_bounded_u64() {
        let maybe_templated: Templated<BoundedU64<0, 1000>> =
            serde_json::from_value(json!(1)).unwrap();
        assert_eq!(maybe_templated.render(&()).unwrap(), 1);
    }

    #[test]
    fn test_maybe_templated_render_bounded_u64() {
        let maybe_templated: Templated<BoundedU64<0, 1000>> =
            serde_json::from_value(json!("{{ rendered }}")).unwrap();
        assert_eq!(
            maybe_templated
                .render(&btreemap! {
                    "rendered" => 1000
                })
                .unwrap(),
            1000
        );
    }

    #[test]
    fn test_render_out_of_bounds_fails() {
        let maybe_templated: Templated<BoundedU64<0, 1000>> =
            serde_json::from_value(json!("{{ rendered }}")).unwrap();
        assert!(
            maybe_templated
                .render(&btreemap! {
                    "rendered" => 1001
                })
                .is_err()
        );
    }

    #[test]
    fn test_maybe_templated_string() {
        // Strings are always parsed as templates
        let maybe_templated: Templated<String> =
            serde_json::from_value(json!("{{ rendered }}")).unwrap();

        assert_eq!(
            maybe_templated
                .render(&btreemap! {
                    "rendered" => 1001
                })
                .unwrap(),
            "1001"
        );
    }
}
