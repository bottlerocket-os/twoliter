//! This module provides useful blanket implemntations for the [`TemplateOf`] trait.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;

use crate::{TemplateOf, TemplatedError};
use serde::Serialize;

impl<T: TemplateOf> TemplateOf for Option<T> {
    type Target = Option<T::Target>;

    fn render(&self, template_context: &impl Serialize) -> Result<Self::Target, TemplatedError> {
        match self {
            Some(t) => Ok(Some(t.render(template_context)?)),
            None => Ok(None),
        }
    }
}

impl<T: TemplateOf> TemplateOf for Vec<T> {
    type Target = Vec<T::Target>;

    fn render(&self, template_context: &impl Serialize) -> Result<Self::Target, TemplatedError> {
        self.iter()
            .map(|item| item.render(template_context))
            .collect()
    }
}

impl<K, V> TemplateOf for HashMap<K, V>
where
    K: TemplateOf,
    V: TemplateOf,
    K::Target: Eq + Hash + Debug,
{
    type Target = HashMap<K::Target, V::Target>;

    fn render(&self, template_context: &impl Serialize) -> Result<Self::Target, TemplatedError> {
        use crate::templated_error::*;

        let rendered: Vec<_> = self
            .iter()
            .map(|(k, v)| Ok((k.render(template_context)?, v.render(template_context)?)))
            .collect::<Result<_, _>>()?;

        let mut unique_keys = HashSet::new();
        for (rendered_key, _) in rendered.iter() {
            snafu::ensure!(
                unique_keys.insert(rendered_key),
                RenderCollisionSnafu {
                    render_result: format!("{rendered_key:?}")
                }
            );
        }

        Ok(rendered.into_iter().collect())
    }
}

impl<K, V> TemplateOf for BTreeMap<K, V>
where
    K: TemplateOf,
    V: TemplateOf,
    K::Target: Ord + Debug,
{
    type Target = BTreeMap<K::Target, V::Target>;

    fn render(&self, template_context: &impl Serialize) -> Result<Self::Target, TemplatedError> {
        use crate::templated_error::*;

        let rendered: Vec<_> = self
            .iter()
            .map(|(k, v)| Ok((k.render(template_context)?, v.render(template_context)?)))
            .collect::<Result<_, _>>()?;

        let mut unique_keys = BTreeSet::new();
        for (rendered_key, _) in rendered.iter() {
            snafu::ensure!(
                unique_keys.insert(rendered_key),
                RenderCollisionSnafu {
                    render_result: format!("{rendered_key:?}")
                }
            );
        }

        Ok(rendered.into_iter().collect())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use maplit::{btreemap, hashmap};
    use test_case::test_case;

    use crate::{TemplateOf, Templated};

    #[test_case(
        btreemap! {
            Templated::raw("a".to_string()) => Templated::raw("a".to_string()),
            Templated::raw("b".to_string()) => Templated::raw("b".to_string()),
            Templated::template("a".to_string()) => Templated::raw("b".to_string()),
        },
        true;
        "two keys collide"
    )]
    #[test_case(
        btreemap! {
            Templated::raw("a".to_string()) => Templated::raw("a".to_string()),
            Templated::raw("b".to_string()) => Templated::raw("b".to_string()),
        },
        false;
        "no collision"
    )]
    fn test_btreemap_collisions(
        template_map: BTreeMap<Templated<String>, Templated<String>>,
        expect_collision: bool,
    ) {
        if expect_collision {
            assert!(matches!(
                template_map.render(&()),
                Err(crate::TemplatedError::RenderCollision { .. })
            ));
        } else {
            assert!(template_map.render(&()).is_ok());
        }
    }

    #[test_case(
        hashmap! {
            Templated::raw("a".to_string()) => Templated::raw("a".to_string()),
            Templated::raw("b".to_string()) => Templated::raw("b".to_string()),
            Templated::template("a".to_string()) => Templated::raw("b".to_string()),
        },
        true;
        "two keys collide"
    )]
    #[test_case(
        hashmap! {
            Templated::raw("a".to_string()) => Templated::raw("a".to_string()),
            Templated::raw("b".to_string()) => Templated::raw("b".to_string()),
        },
        false;
        "no collision"
    )]
    fn test_hashmap_collisions(
        template_map: HashMap<Templated<String>, Templated<String>>,
        expect_collision: bool,
    ) {
        if expect_collision {
            assert!(matches!(
                template_map.render(&()),
                Err(crate::TemplatedError::RenderCollision { .. })
            ));
        } else {
            assert!(template_map.render(&()).is_ok());
        }
    }
}
