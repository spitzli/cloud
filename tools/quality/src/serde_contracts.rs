//! Protect strict internally tagged contracts from Serde's unit-variant field skipping.
//!
//! Conditional attributes are conservatively treated as potentially active without evaluating
//! feature/target predicates. Conditional exemptions (`skip` or adjacent `content`) cannot
//! suppress findings for configurations where those exemptions are absent. Mutually exclusive
//! predicates may therefore produce a finding that requires manual contract review.

use std::collections::BTreeMap;
use syn::{Fields, Item, Meta, Path, Token, UseTree, punctuated::Punctuated, visit::Visit};

#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    pub line: usize,
    pub message: String,
}

pub fn violations(source: &str) -> Result<Vec<Finding>, syn::Error> {
    let file = syn::parse_file(source)?;
    let mut checker = Checker {
        aliases: BTreeMap::from([
            ("serde".into(), "serde".into()),
            ("serde_derive".into(), "serde_derive".into()),
            ("Deserialize".into(), "serde::Deserialize".into()),
        ]),
        findings: Vec::new(),
        error: None,
    };
    checker.imports(&file.items);
    checker.visit_file(&file);
    match checker.error {
        Some(error) => Err(error),
        None => Ok(checker.findings),
    }
}

struct Checker {
    aliases: BTreeMap<String, String>,
    findings: Vec<Finding>,
    error: Option<syn::Error>,
}

impl Checker {
    fn resolve(&self, name: &str) -> String {
        let name = name.strip_prefix("super::").unwrap_or(name);
        let (head, tail) = name.split_once("::").unwrap_or((name, ""));
        let base = self.aliases.get(head).map_or(head, String::as_str);
        match tail {
            "" => base.to_owned(),
            _ => format!("{base}::{tail}"),
        }
    }

    fn imports(&mut self, items: &[Item]) {
        for item in items {
            if let Item::Use(import) = item {
                self.import(&import.tree, "");
            }
        }
    }

    fn import(&mut self, tree: &UseTree, prefix: &str) {
        match tree {
            UseTree::Path(path) => {
                self.import(&path.tree, &format!("{prefix}{}::", path.ident));
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.import(item, prefix);
                }
            }
            UseTree::Name(name) => {
                let path = format!("{prefix}{}", name.ident);
                self.aliases
                    .insert(name.ident.to_string(), self.resolve(&path));
            }
            UseTree::Rename(rename) => {
                let path = match rename.ident == "self" {
                    true => prefix.trim_end_matches("::").to_owned(),
                    false => format!("{prefix}{}", rename.ident),
                };
                self.aliases
                    .insert(rename.rename.to_string(), self.resolve(&path));
            }
            UseTree::Glob(_) => {
                let origin = self.resolve(prefix.trim_end_matches("::"));
                if matches!(origin.as_str(), "serde" | "serde_derive") {
                    self.aliases
                        .insert("Deserialize".into(), format!("{origin}::Deserialize"));
                }
            }
        }
    }

    fn inspect(&mut self, item: &syn::ItemEnum) -> Result<(), syn::Error> {
        let mut deserialize = false;
        let mut internal = false;
        let mut content = false;
        let mut strict = false;
        for (attribute, conditional) in expanded_attributes(&item.attrs)? {
            if attribute.path().is_ident("derive") {
                for path in attribute
                    .require_list()?
                    .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?
                {
                    let name = path
                        .segments
                        .iter()
                        .map(|segment| segment.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    deserialize |= matches!(
                        self.resolve(&name).as_str(),
                        "serde::Deserialize" | "serde_derive::Deserialize"
                    );
                }
            }
            if attribute.path().is_ident("serde") {
                for option in attribute
                    .require_list()?
                    .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
                {
                    internal |= option.path().is_ident("tag");
                    content |= !conditional && option.path().is_ident("content");
                    strict |= option.path().is_ident("deny_unknown_fields");
                }
            }
        }
        if deserialize && internal && strict && !content {
            for variant in &item.variants {
                if matches!(variant.fields, Fields::Unit) && !skips_deserialization(variant)? {
                    self.findings.push(Finding {
                        line: variant.ident.span().start().line,
                        message: format!(
                            "{}::{} is a unit variant in a strict internally tagged Serde enum; use an empty struct variant `{} {{}}` so unknown fields are rejected",
                            item.ident, variant.ident, variant.ident
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

fn expanded_attributes(attributes: &[syn::Attribute]) -> Result<Vec<(Meta, bool)>, syn::Error> {
    fn expand(
        attribute: Meta,
        conditional: bool,
        result: &mut Vec<(Meta, bool)>,
    ) -> Result<(), syn::Error> {
        if attribute.path().is_ident("cfg_attr") {
            let arguments = attribute
                .require_list()?
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            // The first argument is the predicate; remaining attributes inherit conditionality.
            for nested in arguments.into_iter().skip(1) {
                expand(nested, true, result)?;
            }
            return Ok(());
        }
        result.push((attribute, conditional));
        Ok(())
    }

    let mut result = Vec::new();
    for attribute in attributes {
        expand(attribute.meta.clone(), false, &mut result)?;
    }
    Ok(result)
}

fn skips_deserialization(variant: &syn::Variant) -> Result<bool, syn::Error> {
    for (attribute, conditional) in expanded_attributes(&variant.attrs)? {
        if !conditional && attribute.path().is_ident("serde") {
            let options = attribute
                .require_list()?
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            if options.iter().any(|option| {
                option.path().is_ident("skip") || option.path().is_ident("skip_deserializing")
            }) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

impl<'ast> Visit<'ast> for Checker {
    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if let Err(error) = self.inspect(item) {
            self.error = Some(error);
        }
        syn::visit::visit_item_enum(self, item);
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let inherited = self.aliases.clone();
        if let Some((_, items)) = &item.content {
            self.imports(items);
        }
        syn::visit::visit_item_mod(self, item);
        self.aliases = inherited;
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let inherited = self.aliases.clone();
        for statement in &block.stmts {
            if let syn::Stmt::Item(Item::Use(import)) = statement {
                self.import(&import.tree, "");
            }
        }
        syn::visit::visit_block(self, block);
        self.aliases = inherited;
    }
}

#[cfg(test)]
mod tests {
    use super::violations;

    #[test]
    fn inspects_conditional_derives_and_strictness_recursively() {
        let source = r#"
#[cfg_attr(all(), derive(serde::Deserialize))]
#[serde(tag = "kind", deny_unknown_fields)]
enum ConditionalDerive { Run }
#[derive(serde::Deserialize)]
#[serde(tag = "kind")]
#[cfg_attr(feature = "strict", serde(deny_unknown_fields))]
enum ConditionalStrictness { Run }
#[cfg_attr(feature = "outer", cfg_attr(feature = "inner",
    derive(serde::Deserialize), serde(tag = "kind", deny_unknown_fields)))]
enum Nested { Run }
"#;
        assert_eq!(violations(source).unwrap().len(), 3);
    }

    #[test]
    fn conditional_skip_and_content_cannot_exempt_other_configurations() {
        let source = r#"
#[derive(serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum ConditionalSkip {
    #[cfg_attr(feature = "output", serde(skip))]
    Run,
    #[cfg_attr(feature = "outer", cfg_attr(feature = "inner", serde(skip_deserializing)))]
    Stop,
    #[serde(skip_deserializing)]
    NeverRead,
}
#[derive(serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
#[cfg_attr(feature = "adjacent", serde(content = "payload"))]
enum ConditionalContent { Run }
"#;
        let found = violations(source).unwrap();
        assert_eq!(found.len(), 3);
        assert!(
            found
                .iter()
                .any(|finding| finding.message.contains("ConditionalContent::Run"))
        );
        assert!(
            !found
                .iter()
                .any(|finding| finding.message.contains("NeverRead"))
        );
    }

    #[test]
    fn reports_strict_internally_tagged_unit_variants_at_the_variant() {
        let source = r#"#[derive(serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Command {
    #[serde(rename = "run")]
    Run,
    Stop {},
}
"#;
        let found = violations(source).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 5);
        assert!(found[0].message.contains("Command::Run"));
    }

    #[test]
    fn recognizes_renamed_derives_and_nested_serde_options() {
        let source = r#"
use serde::{Deserialize as Decode, self as wire};
#[derive(Decode)]
#[serde(rename(serialize = "Command", deserialize = "Command"))]
#[serde(deny_unknown_fields, tag = "kind")]
enum Command { Run }
mod nested {
    use super::wire;
    #[derive(wire::Deserialize)]
    #[serde(tag = "kind", deny_unknown_fields)]
    enum Nested { Stop }
}
"#;
        assert_eq!(violations(source).unwrap().len(), 2);
    }

    #[test]
    fn checks_module_aliases_without_leaking_them_to_sibling_modules() {
        let source = r#"
mod first {
    use serde::Deserialize as Decode;
    #[derive(Decode)]
    #[serde(tag = "kind", deny_unknown_fields)]
    enum First { Run }
}
mod second {
    use other::Deserialize;
    #[derive(Deserialize)]
    #[serde(tag = "kind", deny_unknown_fields)]
    enum Second { Run }
}
"#;
        let found = violations(source).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("First::Run"));
    }

    #[test]
    fn distinguishes_strict_internal_tags_from_other_contracts() {
        let source = r#"
#[derive(serde::Deserialize)]
#[serde(tag = "kind", content = "payload", deny_unknown_fields)]
enum Adjacent { Run }
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum External { Run }
#[derive(serde::Deserialize)]
#[serde(tag = "kind")]
enum Permissive { Run }
#[derive(serde::Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum OutputOnly { Run }
#[derive(serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Strict { Run {}, Stop { reason: String } }
#[derive(serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Unreachable {
    #[serde(skip)]
    Skipped,
    #[serde(skip_deserializing)]
    OutputOnly,
    Run {},
}
"#;
        assert!(violations(source).unwrap().is_empty());
    }

    #[test]
    fn ignores_comments_strings_and_unrelated_derive_names() {
        let source = r##"
// #[derive(serde::Deserialize)] #[serde(tag="kind", deny_unknown_fields)] enum Bad { Run }
const EXAMPLE: &str = r#"#[derive(serde::Deserialize)]
#[serde(tag="kind", deny_unknown_fields)] enum Bad { Run }"#;
#[derive(other::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Unrelated { Run }
"##;
        assert!(violations(source).unwrap().is_empty());
    }

    #[test]
    fn reports_invalid_rust_instead_of_silently_skipping_it() {
        assert!(violations("enum Broken {").is_err());
    }
}
