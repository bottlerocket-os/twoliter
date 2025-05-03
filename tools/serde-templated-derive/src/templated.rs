use darling::util::{Callable, PathList};
use darling::{FromDeriveInput, FromField};
use quote::{ToTokens, format_ident, quote};
use std::ops::Deref;
use syn::parse_quote;

use crate::container_visitor::visit_container_leaves_of_type;
use crate::{Error, Result};

#[derive(FromDeriveInput)]
#[darling(attributes(templated), supports(struct_named), forward_attrs)]
pub struct TemplatedOpts {
    ident: syn::Ident,
    vis: syn::Visibility,
    generics: syn::Generics,
    attrs: Vec<syn::Attribute>,
    data: darling::ast::Data<(), TemplatedField>,

    /// Place the listed derive macros on the generated struct.
    #[darling(default)]
    derive: PathList,

    /// Forward attributes with the given names
    #[darling(default)]
    forward_attrs: PathList,

    /// Add the given strings as attributes to the generated struct.
    #[darling(default)]
    templated_attrs: Vec<syn::LitStr>,

    /// Skip deriving `Serialize` and `Deserialize` for the generated struct
    #[darling(default)]
    skip_serde_derive: bool,
}

impl TemplatedOpts {
    fn templated_struct_name(&self) -> syn::Ident {
        format_ident!("Templated{}", self.ident)
    }

    pub(crate) fn templated_struct_def(&self) -> Result<proc_macro2::TokenStream> {
        let Self {
            ident: _ident,
            vis,
            generics,
            derive,
            skip_serde_derive,
            // Struct field data is handled in
            data: _data,
            // Attributes are handled in `self.render_struct_attrs()`
            attrs: _attrs,
            forward_attrs: _forward_attrs,
            templated_attrs: _templated_attrs,
        } = self;

        let generated_struct_name = self.templated_struct_name();
        let mut derives: Vec<syn::Path> = derive.deref().clone();

        if !skip_serde_derive {
            derives.push(parse_quote!(::serde::Serialize));
            derives.push(parse_quote!(::serde::Deserialize));
        }

        let attributes = self.render_struct_attrs()?;
        let templated_fields = self.render_templated_fields()?;

        let (_impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        Ok(quote! {
            #[derive(#(#derives,)*)]
            #(#attributes)*
            #vis struct #generated_struct_name #ty_generics #where_clause {
                #(#templated_fields,)*
            }
        })
    }

    /// Defines the `TemplateOf` trait for the resulting `Templated` type.
    pub(crate) fn render_template_of_trait_def(&self) -> Result<proc_macro2::TokenStream> {
        let Self {
            ident, generics, ..
        } = self;

        let generated_struct_name = self.templated_struct_name();
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let render_assignments: Vec<proc_macro2::TokenStream> = self
            .defined_fields()?
            .into_iter()
            .map(TemplatedField::render_assignment)
            .collect::<Result<_, _>>()?;

        Ok(quote! {
            impl #impl_generics ::serde_templated::TemplateOf for #generated_struct_name #ty_generics #where_clause {
                type Target = #ident #ty_generics;

                fn render(&self, template_context: &impl ::serde::Serialize) -> ::std::result::Result<
                    Self::Target,
                    ::serde_templated::TemplatedError
                > {
                    ::std::result::Result::Ok(#ident {
                        #(#render_assignments,)*
                    })
                }
            }
        })
    }

    /// Generate all struct attributes for Templated type.
    ///
    /// This includes any original `attrs` who's identity appears in `forward_attrs`, as well as
    /// any explicit attrs in `templated_attrs`.
    fn render_struct_attrs(&self) -> Result<Vec<proc_macro2::TokenStream>> {
        let Self {
            attrs,
            forward_attrs,
            templated_attrs,
            ..
        } = self;

        let forwarded_attr_idents = path_list_as_ident_list(forward_attrs)?;

        let forwarded_attrs = attrs
            .iter()
            .filter(|attr| match &attr.meta {
                syn::Meta::List(list) => forwarded_attr_idents
                    .iter()
                    .any(|forwarded_ident| list.path.get_ident() == Some(forwarded_ident)),
                _ => true,
            })
            .map(|attr| attr.to_token_stream());

        let templated_attrs: Vec<proc_macro2::TokenStream> = templated_attrs
            .iter()
            .map(|attr_str| {
                attr_str.value().parse().map_err(|e| {
                    Error::custom(format!(
                        "Could not parse '{}' as attribute: {}",
                        attr_str.value(),
                        e
                    ))
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(forwarded_attrs.chain(templated_attrs).collect())
    }

    /// Returns the fields defined on the input struct
    fn defined_fields(&self) -> Result<Vec<&TemplatedField>> {
        let fields = self
            .data
            .as_ref()
            .take_struct()
            .ok_or(Error::custom("Templated not valid for non-structs"))?
            .fields;

        Ok(fields)
    }

    /// Returns field definitions for the resultant `Templated` struct
    fn render_templated_fields(&self) -> Result<Vec<proc_macro2::TokenStream>> {
        self.defined_fields()?
            .into_iter()
            .map(TemplatedField::generate_templated_field)
            .collect()
    }
}

fn path_list_as_ident_list(path_list: &PathList) -> Result<Vec<syn::Ident>> {
    path_list
        .iter()
        .map(|path| {
            path.get_ident()
                .cloned()
                .ok_or(Error::custom("Expected identifier"))
        })
        .collect()
}

#[derive(Debug, FromField, Clone)]
#[darling(attributes(templated), forward_attrs)]
/// The `#[templated]` attribute for fields of a `Templated` struct.
pub struct TemplatedField {
    ident: Option<syn::Ident>,
    vis: syn::Visibility,
    ty: syn::Type,
    attrs: Vec<syn::Attribute>,

    /// Add the given strings as attributes to the generated field
    #[darling(default)]
    templated_attrs: Vec<syn::LitStr>,

    /// Do not turn this field into a Templated field.
    #[darling(default)]
    skip: bool,

    /// Forward attributes with the given names
    #[darling(default)]
    forward_attrs: PathList,

    /// Turn this field into a Templated field, but use the given type as the template.
    ///
    /// Useful if `Templated`'s "leaf generic" template behavior is undesirable.
    /// For example, if you want `Templated<Option<T>>` instead of `Option<Templated<T>>`, you
    /// should override the default behavior via `templated_as`.
    template_as: Option<syn::LitStr>,

    /// Custom function to render the generated template type into the desired type
    render_with: Option<Callable>,
}

impl TemplatedField {
    fn generate_templated_field(&self) -> Result<proc_macro2::TokenStream> {
        let Self {
            ident,
            vis,
            ty,
            skip,
            template_as,
            templated_attrs,
            render_with: _render_with,
            // Attributes are handled in `self.render_field_attrs()`
            attrs: _attrs,
            forward_attrs: _forward_attrs,
        } = self;

        let ident = ident
            .as_ref()
            .ok_or(darling::Error::custom("Missing identifier for field"))?;

        if *skip && template_as.is_some() {
            return Err(darling::Error::custom(
                "`skip` and `template_as` are mutually exclusive options",
            ));
        }

        let attributes = self.render_field_attrs()?;

        if *skip {
            return Ok(quote! {
                #(#attributes)*
                #(#templated_attrs)*
                #vis #ident: #ty
            });
        }

        let final_template_type: proc_macro2::TokenStream =
            if let Some(template_as) = template_as.as_ref() {
                template_as.value().parse().map_err(|e| {
                    Error::custom(format!(
                        "Could not parse '{}' as template type: {}",
                        template_as.value(),
                        e
                    ))
                })?
            } else {
                // For container types we're replacing "contained" data and not the container itself
                // e.g.
                // * Option<Vec<T>> becomes Option<Vec<Templated<T>>>
                // * HashMap<String, String> becomes HashMap<Templated<String>, Templated<String>>
                let mut modified_type = ty.clone();
                visit_container_leaves_of_type(&mut modified_type, &mut |leaf_type| {
                    *leaf_type = parse_quote!(::serde_templated::Templated<#leaf_type>);
                });
                parse_quote!(#modified_type)
            };

        Ok(quote! {
            #(#attributes)*
            #(#templated_attrs)*
            #vis #ident: #final_template_type
        })
    }

    /// Generate all field attributes for Templated type.
    ///
    /// This includes any original `attrs` who's identity appears in `forward_attrs`, as well as
    /// any explicit attrs in `templated_attrs`.
    fn render_field_attrs(&self) -> Result<Vec<proc_macro2::TokenStream>> {
        let Self {
            attrs,
            forward_attrs,
            templated_attrs,
            ..
        } = self;

        let forwarded_attr_idents = path_list_as_ident_list(forward_attrs)?;
        let forwarded_attrs = attrs
            .iter()
            .filter(|attr| match &attr.meta {
                syn::Meta::List(list) => forwarded_attr_idents
                    .iter()
                    .any(|forwarded_ident| list.path.get_ident() == Some(forwarded_ident)),
                _ => true,
            })
            .cloned();

        let templated_attrs: Vec<proc_macro2::TokenStream> = templated_attrs
            .iter()
            .map(|attr_str| {
                attr_str.value().parse().map_err(|e| {
                    Error::custom(format!(
                        "Could not parse '{}' as attribute: {}",
                        attr_str.value(),
                        e
                    ))
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(forwarded_attrs
            .map(|attr| attr.to_token_stream())
            .chain(templated_attrs)
            .collect())
    }

    /// Implements the conversion of the given `Templated` field back into the field of the parent
    /// struct.
    fn render_assignment(&self) -> Result<proc_macro2::TokenStream> {
        let Self {
            ident,
            skip,
            render_with,
            ..
        } = self;

        let field_name = ident
            .as_ref()
            .ok_or(Error::custom("Missing identifier for field"))?
            .to_string();

        Ok(match (render_with, skip) {
            // If the user provided a render function, use that.
            (Some(render_with), _) => {
                quote! {
                    #ident: (#render_with)(&self.#ident, template_context)
                        .map_err(|e| ::serde_templated::TemplatedError::RenderField {
                            field: #field_name.to_string(),
                            source: Box::new(e)
                        })?
                }
            }
            // Skipped fields with no provided render function are passed through verbatim
            (_, true) => {
                quote! {
                    #ident: self.#ident
                }
            }
            // Otherwise, assume the templated field implements `TemplateOf`
            _ => {
                quote! {
                    #ident: ::serde_templated::TemplateOf::render_field(&self.#ident, #field_name, template_context)?
                }
            }
        })
    }
}
