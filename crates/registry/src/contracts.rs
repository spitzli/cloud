//! Public boundary validation. Objects retain absent fields separately from explicit nulls.
use crate::{RegistryError, Result};
use serde::Deserialize;
use serde_json::Value;

pub fn record_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|c| c.is_ascii_digit())
        || value.parse::<i64>().is_err()
    {
        return Err(RegistryError::Invalid("Invalid ID."));
    }
    Ok(())
}
pub(crate) fn trim(value: &str) -> &str {
    value.trim_matches(|c: char| matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'))
}
fn text(value: &mut Value, max: usize, required: bool) -> Result<()> {
    if value.is_null() && !required {
        return Ok(());
    }
    let value_text = value
        .as_str()
        .ok_or(RegistryError::Invalid("Invalid text field."))?;
    let clean = trim(value_text);
    // JavaScript/Zod count UTF-16 code units, including surrogate pairs.
    if clean.encode_utf16().count() > max || required && clean.is_empty() {
        return Err(RegistryError::Invalid("Invalid text field."));
    }
    *value = Value::String(clean.to_owned());
    Ok(())
}
pub fn customer_input(mut value: Value) -> Result<Value> {
    let object = value
        .as_object_mut()
        .ok_or(RegistryError::Invalid("Invalid customer."))?;
    if !object.contains_key("name") {
        return Err(RegistryError::Invalid("Name is required."));
    }
    for (key, value) in object {
        match key.as_str() {
            "name" => text(value, 160, true)?,
            "customerType" if matches!(value.as_str(), Some("person" | "company")) => (),
            "firstName" | "lastName" | "companyName" | "contactName" | "addressLine1"
            | "addressLine2" | "city" | "region" => text(value, 160, false)?,
            "notes" => text(value, 3000, false)?,
            "phone" => text(value, 50, false)?,
            "postalCode" => text(value, 32, false)?,
            "country" => {
                text(value, usize::MAX, false)?;
                if let Some(country) = value.as_str() {
                    let upper = country.to_uppercase();
                    if upper.len() != 2 || !upper.bytes().all(|c| c.is_ascii_uppercase()) {
                        return Err(RegistryError::Invalid("Use a two-letter ISO country code."));
                    }
                    *value = Value::String(upper);
                }
            }
            "contactEmail" => {
                text(value, 254, false)?;
                if let Some(email) = value.as_str() {
                    let valid = email.split_once('@').is_some_and(|(local, domain)| {
                        !local.is_empty()
                            && !local.starts_with('.')
                            && !local.ends_with(['.', '\''])
                            && !local.contains("..")
                            && local
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"_'+-.".contains(&c))
                            && domain.rsplit_once('.').is_some_and(|(_, tld)| {
                                tld.len() >= 2 && tld.bytes().all(|c| c.is_ascii_alphabetic())
                            })
                            && domain.split('.').all(|label| {
                                !label.is_empty()
                                    && !label.starts_with('-')
                                    && label
                                        .bytes()
                                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                            })
                    });
                    if !valid {
                        return Err(RegistryError::Invalid("Invalid email address."));
                    }
                }
            }
            _ => return Err(RegistryError::Invalid("Invalid customer field.")),
        }
    }
    Ok(value)
}
pub fn project_input(mut value: Value) -> Result<Value> {
    let object = value
        .as_object_mut()
        .ok_or(RegistryError::Invalid("Invalid project."))?;
    if !object.contains_key("name") || !object.contains_key("customer") {
        return Err(RegistryError::Invalid("Name and customer are required."));
    }
    for (key, value) in object {
        match key.as_str() {
            "name" => text(value, 160, true)?,
            "customer" => record_id(
                value
                    .as_str()
                    .ok_or(RegistryError::Invalid("Invalid ID."))?,
            )?,
            "notes" => text(value, 3000, false)?,
            "url" | "repositoryURL" => https_url(value)?,
            _ => return Err(RegistryError::Invalid("Invalid project field.")),
        }
    }
    Ok(value)
}
pub fn https_url(value: &Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let parsed = value
        .as_str()
        .and_then(|text| url::Url::parse(text).ok())
        .ok_or(RegistryError::Invalid(
            "Use an HTTPS URL without credentials.",
        ))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
    {
        return Err(RegistryError::Invalid(
            "Use an HTTPS URL without credentials.",
        ));
    }
    Ok(())
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListInput {
    pub collection: String,
    pub search: Option<String>,
    #[serde(default = "one")]
    pub page: u64,
    #[serde(default = "twenty")]
    pub limit: u64,
    pub status: Option<String>,
    pub customer: Option<String>,
    pub target_collection: Option<String>,
    #[serde(rename = "targetID")]
    pub target_id: Option<String>,
    #[serde(default = "default_sort")]
    pub sort: String,
}
fn one() -> u64 {
    1
}
fn twenty() -> u64 {
    20
}
fn default_sort() -> String {
    "-updatedAt".into()
}
impl ListInput {
    pub fn parse(mut value: Value) -> Result<Self> {
        for key in ["page", "limit"] {
            if let Some(number) = value.get(key).and_then(Value::as_f64)
                && number.is_finite()
                && number.fract() == 0.0
                && (0.0..=10000.0).contains(&number)
            {
                value[key] = Value::from(number as u64);
            }
        }
        // Optional fields accept omission, not null, matching the public Zod contract.
        if value
            .as_object()
            .is_some_and(|o| o.values().any(Value::is_null))
        {
            return Err(RegistryError::Invalid("Invalid list query."));
        }
        let mut input: Self = serde_json::from_value(value)
            .map_err(|_| RegistryError::Invalid("Invalid list query."))?;
        if input.collection.is_empty()
            || input.collection.len() > 80
            || !(1..=10000).contains(&input.page)
            || !(1..=50).contains(&input.limit)
            || input.sort.len() > 80
        {
            return Err(RegistryError::Invalid("Invalid pagination."));
        }
        for value in [&input.status, &input.target_collection]
            .into_iter()
            .flatten()
        {
            if value.encode_utf16().count() > 80 {
                return Err(RegistryError::Invalid("Invalid list filter."));
            }
        }
        for id in [&input.customer, &input.target_id].into_iter().flatten() {
            record_id(id)?;
        }
        if let Some(search) = &mut input.search {
            *search = trim(search).to_owned();
            if search.encode_utf16().count() > 160 {
                return Err(RegistryError::Invalid("Invalid search."));
            }
        }
        Ok(input)
    }
}
