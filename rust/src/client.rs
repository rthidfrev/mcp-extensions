//! Typed access to OpenAI client metadata received with an MCP request.
//!
//! See the [OpenAI plugin reference](https://developers.openai.com/plugins/reference#_meta-fields-the-client-provides).
//! These readers borrow request metadata; they do not create identifiers, retain
//! session state or change the request. Fields depend on what the host supplies.

use rmcp::{
    RoleServer, handler::server::tool::ToolCallContext, model::RequestMetaObject,
    service::RequestContext,
};
use serde_json::Value;

use crate::{
    Error,
    validation::{failure, pointer},
};

const SESSION_KEY: &str = "openai/session";
const SUBJECT_KEY: &str = "openai/subject";
const ORGANIZATION_KEY: &str = "openai/organization";
const LOCALE_KEY: &str = "openai/locale";
const LEGACY_LOCALE_KEY: &str = "webplus/i18n";
const USER_AGENT_KEY: &str = "openai/userAgent";
const USER_LOCATION_KEY: &str = "openai/userLocation";

/// A borrowed reader that validates only the client field being requested.
///
/// Missing fields return `Ok(None)`. Present `null` or malformed values return
/// [`Error::ModelValidation`] with a JSON Pointer relative to request `_meta`,
/// without including submitted values. Text is returned unchanged; unknown
/// fields and the original metadata are never modified. Reading one field does
/// not require unrelated optional hints to be valid.
#[derive(Debug, Clone, Copy)]
pub struct ClientMetadataReader<'a> {
    meta: &'a RequestMetaObject,
}

impl<'a> ClientMetadataReader<'a> {
    /// Borrow request metadata without validating it, copying it or retaining
    /// session state. Each accessor validates its own field when called.
    pub fn new(meta: &'a RequestMetaObject) -> Self {
        Self { meta }
    }

    /// Read the anonymized conversation identifier from `openai/session`.
    ///
    /// This differs from the JSON-RPC request ID, MCP transport session and
    /// widget instance ID. Host presence and stability are not guaranteed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if the present value is not a string.
    pub fn session_id(&self) -> Result<Option<&'a str>, Error> {
        optional_string(self.meta.get(SESSION_KEY), &pointer(SESSION_KEY))
    }

    /// Read the anonymized user identifier from `openai/subject` unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if the present value is not a string.
    pub fn user_id(&self) -> Result<Option<&'a str>, Error> {
        optional_string(self.meta.get(SUBJECT_KEY), &pointer(SUBJECT_KEY))
    }

    /// Read the host-provided anonymized organization identifier unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if the present value is not a string.
    pub fn organization_id(&self) -> Result<Option<&'a str>, Error> {
        optional_string(self.meta.get(ORGANIZATION_KEY), &pointer(ORGANIZATION_KEY))
    }

    /// Read `openai/locale`, falling back to `webplus/i18n` only when the modern
    /// key is absent. A malformed modern value never triggers fallback.
    /// The string is not validated as a language tag; no default is inserted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if the selected value is not a string.
    pub fn locale(&self) -> Result<Option<&'a str>, Error> {
        let key = if self.meta.contains_key(LOCALE_KEY) {
            LOCALE_KEY
        } else {
            LEGACY_LOCALE_KEY
        };
        optional_string(self.meta.get(key), &pointer(key))
    }

    /// Read the user-agent hint unchanged. It is unsuitable for reliable host
    /// detection or authorization decisions.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if the present value is not a string.
    pub fn user_agent(&self) -> Result<Option<&'a str>, Error> {
        optional_string(self.meta.get(USER_AGENT_KEY), &pointer(USER_AGENT_KEY))
    }

    /// Read the coarse location hint, borrowing text and normalizing finite
    /// numeric or numeric-string coordinates to `f64`. No range, country or
    /// timezone validation is imposed. The hint is unsuitable for authorization.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if the present location is not an
    /// object, a known text field is not a string or a coordinate is not finite.
    pub fn location(&self) -> Result<Option<ClientLocation<'a>>, Error> {
        self.meta
            .get(USER_LOCATION_KEY)
            .map(ClientLocation::from_value)
            .transpose()
    }
}

/// A parsed, borrowed view of OpenAI client metadata.
///
/// Missing fields remain `None`. Identifiers and text are returned unchanged;
/// their formats are not validated. Unknown metadata remains on the request.
/// Use [`ClientMetadataReader`] when only selected fields need to be valid.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClientMetadata<'a> {
    session_id: Option<&'a str>,
    user_id: Option<&'a str>,
    organization_id: Option<&'a str>,
    locale: Option<&'a str>,
    user_agent: Option<&'a str>,
    location: Option<ClientLocation<'a>>,
}

impl<'a> ClientMetadata<'a> {
    /// Validate all known client fields without copying strings or modifying metadata.
    /// Choose this strict parse deliberately when every known field must be valid;
    /// [`ClientMetadataReader`] reads selected fields independently.
    ///
    /// `openai/locale` takes precedence over `webplus/i18n`; the legacy alias is
    /// read only when the modern key is absent. A malformed modern value does not
    /// trigger fallback. Location coordinates accept finite numbers or numeric
    /// strings, which are normalized to `f64` as an SDK interoperability policy.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ModelValidation`] if any known field has the wrong type or a
    /// coordinate cannot be decoded as a finite number. An explicit `null` is a
    /// malformed value, not an absent field. [`Error::path`] identifies the field
    /// with a JSON Pointer relative to request `_meta`, without including its
    /// value. The caller decides how to handle malformed
    /// optional hints; this reader does not reject requests on its own.
    pub fn from_meta(meta: &'a RequestMetaObject) -> Result<Self, Error> {
        let fields = ClientMetadataReader::new(meta);
        Ok(Self {
            session_id: fields.session_id()?,
            user_id: fields.user_id()?,
            organization_id: fields.organization_id()?,
            locale: fields.locale()?,
            user_agent: fields.user_agent()?,
            location: fields.location()?,
        })
    }

    /// Anonymized conversation identifier from `openai/session`.
    ///
    /// This correlates tool calls in one conversation. It is distinct from the
    /// JSON-RPC request ID, the MCP transport session and a widget instance ID.
    /// Its presence or stability on Codex global/thread launches is not established.
    pub fn session_id(&self) -> Option<&'a str> {
        self.session_id
    }

    /// Anonymized user identifier from `openai/subject`.
    pub fn user_id(&self) -> Option<&'a str> {
        self.user_id
    }

    /// Anonymized organization identifier, when the host provides it.
    pub fn organization_id(&self) -> Option<&'a str> {
        self.organization_id
    }

    /// Requested BCP 47 locale, including the legacy `webplus/i18n` fallback.
    ///
    /// The string is not validated as a language tag, and no default is inserted.
    pub fn locale(&self) -> Option<&'a str> {
        self.locale
    }

    /// Optional user-agent hint for formatting or analytics.
    ///
    /// This is not a reliable host detector or an authorization credential.
    pub fn user_agent(&self) -> Option<&'a str> {
        self.user_agent
    }

    /// Optional coarse location hint, unsuitable for authorization decisions.
    pub fn location(&self) -> Option<&ClientLocation<'a>> {
        self.location.as_ref()
    }
}

/// The host's optional coarse user-location fields.
///
/// Text borrows the request; coordinate values are normalized to `f64`. Missing
/// fields remain `None`. No coordinate range, country or timezone validation is
/// imposed. Additional location fields remain on the original request.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClientLocation<'a> {
    city: Option<&'a str>,
    region: Option<&'a str>,
    country: Option<&'a str>,
    timezone: Option<&'a str>,
    longitude: Option<f64>,
    latitude: Option<f64>,
}

impl<'a> ClientLocation<'a> {
    fn from_value(value: &'a Value) -> Result<Self, Error> {
        let path = pointer(USER_LOCATION_KEY);
        let fields = value
            .as_object()
            .ok_or_else(|| failure(&path, "must be an object"))?;
        Ok(Self {
            city: optional_string(fields.get("city"), &format!("{path}/city"))?,
            region: optional_string(fields.get("region"), &format!("{path}/region"))?,
            country: optional_string(fields.get("country"), &format!("{path}/country"))?,
            timezone: optional_string(fields.get("timezone"), &format!("{path}/timezone"))?,
            longitude: optional_coordinate(fields.get("longitude"), &format!("{path}/longitude"))?,
            latitude: optional_coordinate(fields.get("latitude"), &format!("{path}/latitude"))?,
        })
    }

    /// Approximate city supplied by the host.
    pub fn city(&self) -> Option<&'a str> {
        self.city
    }

    /// Approximate region supplied by the host.
    pub fn region(&self) -> Option<&'a str> {
        self.region
    }

    /// Country string supplied by the host, without code validation.
    pub fn country(&self) -> Option<&'a str> {
        self.country
    }

    /// Timezone string supplied by the host, without name validation.
    pub fn timezone(&self) -> Option<&'a str> {
        self.timezone
    }

    /// Approximate longitude as a finite number, without range validation.
    pub fn longitude(&self) -> Option<f64> {
        self.longitude
    }

    /// Approximate latitude as a finite number, without range validation.
    pub fn latitude(&self) -> Option<f64> {
        self.latitude
    }
}

/// Convenience access to OpenAI metadata on rmcp server request contexts.
///
/// Implemented for [`RequestContext<RoleServer>`] and [`ToolCallContext`]. The
/// same reader applies to ordinary calls and MRTR callbacks; it does not depend
/// on a protocol version or an entrypoint type.
///
/// ```
/// use openai_mcp_extensions::{Error, OpenAIRequestContextExt};
/// use rmcp::{RoleServer, service::RequestContext};
///
/// fn conversation_id(context: &RequestContext<RoleServer>) -> Result<Option<&str>, Error> {
///     context.openai_client_fields().session_id()
/// }
/// ```
pub trait OpenAIRequestContextExt {
    /// Borrow this request's metadata without validating it. The returned reader
    /// validates each requested field independently when its accessor is called.
    fn openai_client_fields(&self) -> ClientMetadataReader<'_>;

    /// Validate every known host-provided client field in this request's metadata.
    /// Deliberately choose this strict parse when all fields must be valid; a
    /// malformed optional hint can prevent access to an unrelated valid field.
    ///
    /// # Errors
    ///
    /// Returns the validation errors described by [`ClientMetadata::from_meta`].
    fn openai_client(&self) -> Result<ClientMetadata<'_>, Error>;
}

impl OpenAIRequestContextExt for RequestContext<RoleServer> {
    fn openai_client_fields(&self) -> ClientMetadataReader<'_> {
        ClientMetadataReader::new(&self.meta)
    }

    fn openai_client(&self) -> Result<ClientMetadata<'_>, Error> {
        ClientMetadata::from_meta(&self.meta)
    }
}

impl<S> OpenAIRequestContextExt for ToolCallContext<'_, S> {
    fn openai_client_fields(&self) -> ClientMetadataReader<'_> {
        self.request_context.openai_client_fields()
    }

    fn openai_client(&self) -> Result<ClientMetadata<'_>, Error> {
        self.request_context.openai_client()
    }
}

fn optional_string<'a>(value: Option<&'a Value>, path: &str) -> Result<Option<&'a str>, Error> {
    value
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| failure(path, "must be a string"))
        })
        .transpose()
}

fn optional_coordinate(value: Option<&Value>, path: &str) -> Result<Option<f64>, Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|number| number.is_finite())
    .ok_or_else(|| failure(path, "must be a finite number or numeric string"))?;
    Ok(Some(number))
}
