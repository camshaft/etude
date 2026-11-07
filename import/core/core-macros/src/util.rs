// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use syn::{
    Attribute, Path,
    parse::{Error, Result},
};

/// Extract the crate path from attributes like `#[wire(crate = path)]` or `#[variant(crate = path)]`
///
/// # Arguments
/// * `attrs` - The attributes to search through (will be modified to remove processed attributes)
/// * `attr_name` - The attribute name to look for (e.g., "wire", "variant")
/// * `default_path` - The default path to use if no override is found
pub(crate) fn crate_path(
    attrs: &mut Vec<Attribute>,
    attr_name: &str,
    default_path: Path,
) -> Result<Path> {
    let mut crate_path = None;
    let mut errors: Option<Error> = None;

    attrs.retain(|attr| {
        if !attr.path().is_ident(attr_name) {
            return true;
        }
        if let Err(err) = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                if crate_path.is_some() {
                    return Err(meta.error(format!("duplicate {} crate attribute", attr_name)));
                }
                let path = meta.value()?.call(Path::parse_mod_style)?;
                crate_path = Some(path);
                Ok(())
            } else {
                // Allow other attributes within the same attribute group
                // They'll be handled by the specific derive macro
                Ok(())
            }
        }) {
            match &mut errors {
                None => errors = Some(err),
                Some(errors) => errors.combine(err),
            }
        }
        // Keep the attribute if we didn't successfully parse it
        // or if it contains other nested attributes
        false
    });

    match errors {
        None => Ok(crate_path.unwrap_or(default_path)),
        Some(errors) => Err(errors),
    }
}
