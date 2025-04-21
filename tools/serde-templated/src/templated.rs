use serde::{Deserialize, Serialize, de::DeserializeOwned};
use snafu::ResultExt;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use crate::{Template, TemplateOf, TemplatedError, templated_error};

/// A serde-compatible [`TemplateOf`] type.
///
/// The intent is that this is eventually resolved to some type `T`.
///
/// If the incoming type is a string, this will always assume that it is a template.
/// To resolve this to type `T`, we render the string as a template, and then use `T`'s `FromStr`
/// implementation to finally create a `T`.
///
/// If the incoming type is not a string, we assume that it is a `T` and try to parse it as such.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(untagged, bound = "T: Serialize + DeserializeOwned")]
pub enum Templated<T: FromStr> {
    Template(Template),
    Raw(T),
}

impl<T: FromStr + Clone> Templated<T> {
    pub fn template(value: impl Into<String>) -> Self {
        Self::Template(Template::new(value.into()))
    }

    pub fn raw(value: T) -> Self {
        Self::Raw(value)
    }
}

impl<T, E> TemplateOf for Templated<T>
where
    E: std::error::Error + Send + Sync + 'static,
    T: FromStr<Err = E> + Clone,
{
    type Target = T;

    fn render(&self, template_context: &impl Serialize) -> Result<T, TemplatedError> {
        use templated_error::*;

        match self {
            Templated::Template(templated) => {
                let rendered_str = templated.render(template_context)?;

                T::from_str(rendered_str.as_ref())
                    .boxed()
                    .context(InvalidValueSnafu {
                        value: rendered_str.clone(),
                    })
            }
            Templated::Raw(raw) => Ok(raw.clone()),
        }
    }
}

impl<T> Default for Templated<T>
where
    T: FromStr + Clone + Default,
{
    fn default() -> Self {
        Self::raw(T::default())
    }
}

impl<T> PartialOrd for Templated<T>
where
    T: FromStr + Clone + PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Templated::Template(t1), Templated::Template(t2)) => t1.partial_cmp(t2),
            (Templated::Raw(r1), Templated::Raw(r2)) => r1.partial_cmp(r2),
            (Templated::Template(_), Templated::Raw(_)) => Some(Ordering::Less),
            (Templated::Raw(_), Templated::Template(_)) => Some(Ordering::Greater),
        }
    }
}

impl<T> Ord for Templated<T>
where
    T: FromStr + Clone + Ord,
{
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Templated::Template(t1), Templated::Template(t2)) => t1.cmp(t2),
            (Templated::Raw(r1), Templated::Raw(r2)) => r1.cmp(r2),
            (Templated::Template(_), Templated::Raw(_)) => Ordering::Less,
            (Templated::Raw(_), Templated::Template(_)) => Ordering::Greater,
        }
    }
}

impl<T> Hash for Templated<T>
where
    T: FromStr + Clone + Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        match self {
            Templated::Template(t) => t.hash(state),
            Templated::Raw(r) => r.hash(state),
        }
    }
}

impl<T> From<Template> for Templated<T>
where
    T: FromStr + Clone,
{
    fn from(value: Template) -> Self {
        Self::Template(value)
    }
}
