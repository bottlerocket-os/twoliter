use darling::FromDeriveInput;
use darling::util::SpannedValue;
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

pub(crate) mod container_visitor;
mod templated;

pub(crate) type Error = darling::Error;
pub(crate) type Result<T, E = crate::Error> = std::result::Result<T, E>;

#[proc_macro_derive(Templated, attributes(templated))]
pub fn derive_templated(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    impl_derive_templated(input).map_or_else(|err| err.write_errors().into(), Into::into)
}

fn impl_derive_templated(
    input: syn::DeriveInput,
) -> Result<proc_macro2::TokenStream, darling::Error> {
    let opts: SpannedValue<templated::TemplatedOpts> = SpannedValue::from_derive_input(&input)?;

    let templated_struct = opts.templated_struct_def()?;
    let render_impl = opts.render_template_of_trait_def()?;

    Ok(quote! {
        #templated_struct

        #render_impl
    })
}
