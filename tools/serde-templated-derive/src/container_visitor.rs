//! Provides the [`visit_container_leaves_of_type`] function, which can be used to get a mutable
//! reference to the types "contained" by common containers like `Option` and `Vec`
use syn::parse_quote;

/// Calls the input `visitor` function for all "contained" types.
///
/// For examples of what we consider leaves:
/// * Option<String> -> `String` is the contained type
/// * HashMap<K, V> -> `K` and `V` are contained types
/// * Option<HashMap<K, V>> -> `K` and `V` are contained types
///
/// Because macros do not have access to type information, this is done on a
/// best-effort using the `idents` present in our macro input.
///
/// Container types include:
/// * std::option::Option
/// * std::vec::Vec
/// * std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque}
pub(crate) fn visit_container_leaves_of_type(
    ty: &mut syn::Type,
    visitor: &mut impl FnMut(&mut syn::Type),
) {
    match ty {
        syn::Type::Group(type_group) => {
            visit_container_leaves_of_type(&mut type_group.elem, visitor);
        }
        syn::Type::Path(type_path) => {
            if is_container_type(&type_path.path) {
                // Get the generic args for the type and visit them
                if let Some(seg) = type_path.path.segments.last_mut() {
                    if let syn::PathArguments::AngleBracketed(generic_args) = &mut seg.arguments {
                        for generic_arg in &mut generic_args.args {
                            if let syn::GenericArgument::Type(inner_ty) = generic_arg {
                                visit_container_leaves_of_type(inner_ty, visitor);
                            }
                        }
                    }
                }
            } else {
                visitor(ty);
            }
        }
        _ => {}
    }
}

fn type_path_looks_like(subject: &syn::Path, similar_to: &syn::Path) -> bool {
    let subject_segments = subject.segments.iter().collect::<Vec<_>>();
    let similar_to_segments = similar_to.segments.iter().collect::<Vec<_>>();
    if subject_segments.len() > similar_to_segments.len() {
        return false;
    }

    let length_diff = similar_to_segments.len() - subject_segments.len();
    subject_segments
        .iter()
        .zip(&similar_to_segments[length_diff..])
        .all(|(subject_seg, similar_to_seg)| subject_seg.ident == similar_to_seg.ident)
}

fn is_container_type(ty: &syn::Path) -> bool {
    let container_types: &[syn::Path] = &[
        parse_quote!(::std::option::Option),
        parse_quote!(::std::vec::Vec),
        parse_quote!(::std::collections::BTreeMap),
        parse_quote!(::std::collections::BTreeSet),
        parse_quote!(::std::collections::BinaryHeap),
        parse_quote!(::std::collections::HashMap),
        parse_quote!(::std::collections::HashSet),
        parse_quote!(::std::collections::LinkedList),
        parse_quote!(::std::collections::VecDeque),
    ];
    container_types
        .iter()
        .any(|container_type| type_path_looks_like(ty, container_type))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashSet;
    use test_case::test_case;

    #[test_case(
        parse_quote!(::std::option::Option),
        parse_quote!(::std::option::Option),
        true;
        "exact match"
    )]
    #[test_case(
        parse_quote!(std::option::Option),
        parse_quote!(::std::option::Option),
        true;
        "subject doesn't have leading colon"
    )]
    #[test_case(
        parse_quote!(Option),
        parse_quote!(::std::option::Option),
        true;
        "subject appears to have used a `use` statement"
    )]
    #[test_case(
        parse_quote!(Result<T, E>),
        parse_quote!(::std::result::Result),
        true;
        "subject has generic arguments"
    )]
    #[test_case(
        parse_quote!(::a::b::C),
        parse_quote!(::a::b::E),
        false;
        "they're just different types"
    )]
    fn test_type_path_looks_like(subject: syn::Path, similar_to: syn::Path, expected: bool) {
        assert_eq!(type_path_looks_like(&subject, &similar_to), expected);
    }

    #[test_case(
        parse_quote!(::std::option::Option<String>),
        true;
        "fully-qualified option"
    )]
    #[test_case(
        parse_quote!(Vec<String>),
        true;
        "unqualified Vec"
    )]
    #[test_case(
        parse_quote!(collections::VecDeque<String>),
        true;
        "partially-qualified VecDeque"
    )]
    #[test_case(
        parse_quote!(SomeOtherType<String>),
        false;
        "not a container"
    )]
    #[test_case(
        parse_quote!(nonstd::HashSet<String>),
        false;
        "Same name as HashSet, but different module"
    )]
    fn test_is_container_type(subject: syn::Path, expected: bool) {
        assert_eq!(is_container_type(&subject), expected);
    }

    #[test_case(
        parse_quote!(Option<String>),
        &[parse_quote!(String)];
        "fully-qualified option"
    )]
    #[test_case(
        parse_quote!(HashMap<Key, std::vec::Vec<Value>>),
        &[parse_quote!(Key), parse_quote!(Value)];
        "multiple containers"
    )]
    #[test_case(
        parse_quote!(collections::HashMap<BTreeMap<Option<Key>, std::vec::Vec<Value>>, V>),
        &[parse_quote!(Key), parse_quote!(Value), parse_quote!(V)];
        "deeply tiered containers"
    )]
    #[test_case(
        parse_quote!(NonContainer<T>),
        &[parse_quote!(NonContainer<T>)];
        "not a container"
    )]
    fn test_visit_container_leaves_of_type(mut subject: syn::Type, visited: &[syn::Type]) {
        let expected_visited = visited.into_iter().cloned().collect::<HashSet<_>>();

        let mut visited = HashSet::new();
        let mut visitor = |ty: &mut syn::Type| {
            visited.insert(ty.clone());
        };

        visit_container_leaves_of_type(&mut subject, &mut visitor);

        assert_eq!(expected_visited, visited);
    }
}
