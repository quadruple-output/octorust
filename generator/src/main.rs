mod template;

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use http::status::StatusCode;
use openapiv3::OpenAPI;
use serde::Deserialize;

fn save<P>(p: P, data: &str) -> Result<()>
where
    P: AsRef<Path>,
{
    let p = p.as_ref();
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(p)?;
    f.write_all(data.as_bytes())?;
    f.flush()?;
    Ok(())
}

fn load<P, T>(p: P) -> Result<T>
where
    P: AsRef<Path>,
    for<'de> T: Deserialize<'de>,
{
    let p = p.as_ref();
    let f = File::open(p)?;
    if let Some(ext) = p.extension() {
        if ext == OsStr::new("yaml") || ext == OsStr::new("yml") {
            return Ok(serde_yaml::from_reader(f)?);
        }
    }
    Ok(serde_json::from_reader(f)?)
}

fn load_api<P>(p: P) -> Result<OpenAPI>
where
    P: AsRef<Path>,
{
    let api: OpenAPI = load(p)?;

    if api.openapi != "3.0.3" {
        /*
         * XXX During development we are being very strict, but this should
         * probably be relaxed.
         */
        println!("unexpected version {}", api.openapi);
    }

    if !api.servers.is_empty() {
        println!("servers not presently supported");
    }

    if api.security.is_some() {
        println!("security not presently supported");
    }

    if !api.tags.is_empty() {
        println!("tags not presently supported");
    }

    if let Some(components) = api.components.as_ref() {
        if !components.security_schemes.is_empty() {
            println!("component security schemes not supported");
        }

        if !components.responses.is_empty() {
            println!("component responses not supported");
        }

        if !components.parameters.is_empty() {
            println!("component parameters not supported");
        }

        if !components.request_bodies.is_empty() {
            println!("component request bodies not supported");
        }

        if !components.headers.is_empty() {
            println!("component headers not supported");
        }

        if !components.links.is_empty() {
            println!("component links not supported");
        }

        if !components.callbacks.is_empty() {
            println!("component callbacks not supported");
        }

        /*
         * XXX Ignoring "examples" and "extensions" for now.
         * Explicitly allowing "schemas" through.
         */
    }

    /*
     * XXX Ignoring "external_docs" and "extensions" for now, as they seem not
     * to immediately affect our code generation.
     */

    let mut opids = HashSet::new();
    for p in api.paths.iter() {
        match p.1 {
            openapiv3::ReferenceOr::Reference { reference: _ } => {
                bail!("path {} uses reference, unsupported", p.0);
            }
            openapiv3::ReferenceOr::Item(item) => {
                /*
                 * Make sure every operation has an operation ID, and that each
                 * operation ID is only used once in the document.
                 */
                let mut id = |o: Option<&openapiv3::Operation>| -> Result<()> {
                    if let Some(o) = o {
                        if let Some(oid) = o.operation_id.as_ref() {
                            if !opids.insert(oid.to_string()) {
                                bail!("duplicate operation ID: {}", oid);
                            }

                            if !o.tags.is_empty() {
                                println!("op {}: tags, unsupported", oid);
                            }

                            if !o.servers.is_empty() {
                                println!("op {}: servers, unsupported", oid);
                            }

                            if o.security.is_some() {
                                println!("op {}: security, unsupported", oid);
                            }

                            if o.responses.default.is_some() {
                                bail!("op {}: has response default", oid);
                            }
                        } else {
                            bail!("path {} is missing operation ID", p.0);
                        }
                    }

                    Ok(())
                };

                id(item.get.as_ref())?;
                id(item.put.as_ref())?;
                id(item.post.as_ref())?;
                id(item.delete.as_ref())?;
                id(item.options.as_ref())?;
                id(item.head.as_ref())?;
                id(item.patch.as_ref())?;
                id(item.trace.as_ref())?;

                if !item.servers.is_empty() {
                    bail!("path {} has servers; unsupported", p.0);
                }
            }
        }
    }

    Ok(api)
}

trait ParameterDataExt {
    fn render_type(&self) -> Result<String>;
}

impl ParameterDataExt for openapiv3::ParameterData {
    fn render_type(&self) -> Result<String> {
        use openapiv3::{SchemaKind, Type};

        Ok(match &self.format {
            openapiv3::ParameterSchemaOrContent::Schema(s) => {
                if let Ok(s) = s.item() {
                    match &s.schema_kind {
                        SchemaKind::Type(Type::Boolean {}) => "bool".to_string(),
                        SchemaKind::Type(Type::Array(at)) => {
                            // TODO: actually get the array type.
                            println!("XXX array type: {:?}", at);
                            "&[String]".to_string()
                        }
                        SchemaKind::Type(Type::String(st)) => {
                            use openapiv3::{
                                StringFormat::Date,
                                StringFormat::DateTime,
                                VariantOrUnknownOrEmpty::{Empty, Item, Unknown},
                            };

                            if st.pattern.is_some() {
                                bail!("XXX pattern");
                            }
                            if !st.enumeration.is_empty() {
                                // TODO: figure out enums
                                println!("XXX enumeration");
                            }
                            if st.min_length.is_some() || st.max_length.is_some() {
                                bail!("XXX min/max length");
                            }

                            match &st.format {
                                Item(DateTime) => "DateTime<Utc>".to_string(),
                                Item(Date) => "NaiveDate".to_string(),
                                Empty => "&str".to_string(),
                                Unknown(f) => match f.as_str() {
                                    "float" => "f64".to_string(),
                                    "uri" => "&str".to_string(),
                                    "uri-template" => "&str".to_string(),
                                    "email" => "&str".to_string(),
                                    f => {
                                        bail!("XXX unknown string format {}", f)
                                    }
                                },
                                x => {
                                    bail!("XXX string format {:?}", x);
                                }
                            }
                        }
                        SchemaKind::Type(Type::Integer(it)) => {
                            let mut uint;
                            let width;

                            use openapiv3::VariantOrUnknownOrEmpty::Unknown;
                            if let Unknown(f) = &it.format {
                                match f.as_str() {
                                    "uint" | "uint32" => {
                                        uint = true;
                                        width = 32;
                                    }
                                    "uint64" => {
                                        uint = true;
                                        width = 32;
                                    }
                                    f => bail!("XXX unknown integer format {}", f),
                                }
                            } else {
                                // The format was empty, let's assume it's just a normal
                                // i64.
                                uint = false;
                                width = 64;
                            }

                            if it.multiple_of.is_some() {
                                bail!("XXX multiple_of");
                            }
                            if it.exclusive_minimum || it.exclusive_maximum {
                                bail!("XXX exclusive");
                            }

                            if let Some(min) = it.minimum {
                                if min == 0 {
                                    uint = true;
                                } else {
                                    bail!("XXX invalid minimum: {}", min);
                                }
                            }

                            if it.maximum.is_some() {
                                bail!("XXX maximum");
                            }
                            if !it.enumeration.is_empty() {
                                bail!("XXX enumeration");
                            }
                            if uint {
                                format!("u{}", width)
                            } else {
                                format!("i{}", width)
                            }
                        }
                        openapiv3::SchemaKind::OneOf { one_of } => {
                            // TODO: make this smarter, but for now just make it a string.
                            println!("oneof parameter: {:?}", one_of);
                            "&str".to_string()
                        }
                        x => bail!("unexpected type {:#?}", x),
                    }
                } else {
                    // We have a reference to a type. We could handle it, but for now
                    // easier to return a string.
                    // TODO: handle in the future.
                    "&str".to_string()
                }
            }
            x => bail!("XXX param format {:#?}", x),
        })
    }
}

trait ExtractJsonMediaType {
    fn is_binary(&self) -> Result<bool>;
    fn content_json(&self) -> Result<openapiv3::MediaType>;
}

impl ExtractJsonMediaType for openapiv3::Response {
    fn content_json(&self) -> Result<openapiv3::MediaType> {
        // We do not need to check the length of the content because there might be
        // more than one. For example, if xml or some other format is also defined.
        if let Some(mt) = self.content.get("application/json") {
            Ok(mt.clone())
        } else {
            bail!(
                "could not find application/json, only found {}",
                self.content.keys().next().unwrap()
            );
        }
    }

    fn is_binary(&self) -> Result<bool> {
        if self.content.is_empty() {
            /*
             * XXX If there are no content types, I guess it is not binary?
             */
            return Ok(false);
        }

        // We do not need to check the length of the content because there might be
        // more than one. For example, if xml or some other format is also defined.
        if let Some(mt) = self.content.get("application/octet-stream") {
            if !mt.encoding.is_empty() {
                bail!("XXX encoding");
            }

            if let Some(s) = &mt.schema {
                use openapiv3::{SchemaKind, StringFormat, Type, VariantOrUnknownOrEmpty::Item};

                let s = s.item()?;
                if s.schema_data.nullable {
                    bail!("XXX nullable binary?");
                }
                if s.schema_data.default.is_some() {
                    bail!("XXX default binary?");
                }
                if s.schema_data.discriminator.is_some() {
                    bail!("XXX binary discriminator?");
                }
                match &s.schema_kind {
                    SchemaKind::Type(Type::String(st)) => {
                        if st.min_length.is_some() || st.max_length.is_some() {
                            bail!("binary min/max length");
                        }
                        if !matches!(st.format, Item(StringFormat::Binary)) {
                            bail!("expected binary format string, got {:?}", st.format);
                        }
                        if st.pattern.is_some() {
                            bail!("XXX pattern");
                        }
                        if !st.enumeration.is_empty() {
                            bail!("XXX enumeration");
                        }
                        return Ok(true);
                    }
                    x => {
                        bail!("XXX schemakind type {:?}", x);
                    }
                }
            } else {
                bail!("binary thing had no schema?");
            }
        }

        Ok(false)
    }
}

impl ExtractJsonMediaType for openapiv3::RequestBody {
    fn content_json(&self) -> Result<openapiv3::MediaType> {
        // We do not need to check the length of the content because there might be
        // more than one. For example, if xml or some other format is also defined.
        if let Some(mt) = self.content.get("application/json") {
            Ok(mt.clone())
        } else {
            bail!(
                "could not find application/json, only found {}",
                self.content.keys().next().unwrap()
            );
        }
    }

    fn is_binary(&self) -> Result<bool> {
        if self.content.is_empty() {
            /*
             * XXX If there are no content types, I guess it is not binary?
             */
            return Ok(false);
        }

        // We do not need to check the length of the content because there might be
        // more than one. For example, if xml or some other format is also defined.
        if let Some(mt) = self.content.get("application/octet-stream") {
            if !mt.encoding.is_empty() {
                bail!("XXX encoding");
            }

            if let Some(s) = &mt.schema {
                use openapiv3::{SchemaKind, StringFormat, Type, VariantOrUnknownOrEmpty::Item};

                let s = s.item()?;
                if s.schema_data.nullable {
                    bail!("XXX nullable binary?");
                }
                if s.schema_data.default.is_some() {
                    bail!("XXX default binary?");
                }
                if s.schema_data.discriminator.is_some() {
                    bail!("XXX binary discriminator?");
                }
                match &s.schema_kind {
                    SchemaKind::Type(Type::String(st)) => {
                        if st.min_length.is_some() || st.max_length.is_some() {
                            bail!("binary min/max length");
                        }
                        if !matches!(st.format, Item(StringFormat::Binary)) {
                            bail!("expected binary format string, got {:?}", st.format);
                        }
                        if st.pattern.is_some() {
                            bail!("XXX pattern");
                        }
                        if !st.enumeration.is_empty() {
                            bail!("XXX enumeration");
                        }
                        return Ok(true);
                    }
                    x => {
                        bail!("XXX schemakind type {:?}", x);
                    }
                }
            } else {
                bail!("binary thing had no schema?");
            }
        }

        Ok(false)
    }
}

trait ReferenceOrExt<T> {
    fn item(&self) -> Result<&T>;
}

impl<T> ReferenceOrExt<T> for openapiv3::ReferenceOr<T> {
    fn item(&self) -> Result<&T> {
        match self {
            openapiv3::ReferenceOr::Item(i) => Ok(i),
            openapiv3::ReferenceOr::Reference { reference } => {
                bail!("reference not supported here: {}", reference);
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Clone)]
enum TypeDetails {
    Unknown,
    Basic,
    Enum(TypeId),
    Array(TypeId),
    Optional(TypeId),
    /*
     * Object property names are sorted lexicographically to ensure a stable
     * order in the generated code.
     */
    Object(BTreeMap<String, TypeId>),
}

#[derive(Debug)]
struct TypeEntry {
    id: TypeId,
    name: Option<String>,
    details: TypeDetails,
}

#[derive(Debug, Eq, Clone)]
struct TypeId(u64);

impl PartialOrd for TypeId {
    fn partial_cmp(&self, other: &TypeId) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TypeId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialEq for TypeId {
    fn eq(&self, other: &TypeId) -> bool {
        self.0 == other.0
    }
}

#[derive(Debug)]
struct TypeSpace {
    next_id: u64,
    /*
     * Object types generally have a useful name, which we would like to match
     * with anywhere that name appears in the definition document.  Many other
     * types, though, do not; e.g., an array of strings is just going to become
     * Vec<String> without necesssarily having a useful distinct type name.
     */
    name_to_id: BTreeMap<String, TypeId>,
    id_to_entry: BTreeMap<TypeId, TypeEntry>,
    ref_to_id: BTreeMap<String, TypeId>,

    import_chrono: bool,
}

impl TypeSpace {
    fn new() -> TypeSpace {
        TypeSpace {
            next_id: 1,
            name_to_id: BTreeMap::new(),
            id_to_entry: BTreeMap::new(),
            ref_to_id: BTreeMap::new(),
            import_chrono: false,
        }
    }

    /**
     * Emit a human-readable diagnostic description for this type ID.
     */
    fn describe(&self, tid: &TypeId) -> String {
        if let Some(te) = self.id_to_entry.get(&tid) {
            match &te.details {
                TypeDetails::Basic => {
                    if let Some(n) = &te.name {
                        n.to_string()
                    } else {
                        format!("[BASIC {} !NONAME?]", tid.0)
                    }
                }
                TypeDetails::Enum(itid) => {
                    if let Some(ite) = self.id_to_entry.get(&itid) {
                        if let Some(n) = &ite.name {
                            return format!("enum of {} <{}>", n, itid.0);
                        }
                    }

                    /*
                     * If there is no name attached, we should try a
                     * recursive describe.
                     */
                    format!("enum of {}", self.describe(itid))
                }
                TypeDetails::Array(itid) => {
                    if let Some(ite) = self.id_to_entry.get(&itid) {
                        if let Some(n) = &ite.name {
                            return format!("array of {} <{}>", n, itid.0);
                        }
                    }

                    /*
                     * If there is no name attached, we should try a
                     * recursive describe.
                     */
                    format!("array of {}", self.describe(itid))
                }
                TypeDetails::Optional(itid) => {
                    if let Some(ite) = self.id_to_entry.get(&itid) {
                        if let Some(n) = &ite.name {
                            return format!("option of {} <{}>", n, itid.0);
                        }
                    }

                    /*
                     * If there is no name attached, we should try a
                     * recursive describe.
                     */
                    format!("option of {}", self.describe(itid))
                }
                TypeDetails::Object(_) => {
                    if let Some(n) = &te.name {
                        format!("object {}", n)
                    } else {
                        format!("[OBJECT {} !NONAME?]", tid.0)
                    }
                }
                TypeDetails::Unknown => {
                    format!("[UNKNOWN {}]", tid.0)
                }
            }
        } else {
            format!("[UNMAPPED {}]", tid.0)
        }
    }

    fn render_type(&self, tid: &TypeId, in_mod: bool) -> Result<String> {
        if let Some(te) = self.id_to_entry.get(&tid) {
            match &te.details {
                TypeDetails::Basic => {
                    if let Some(n) = &te.name {
                        Ok(n.to_string())
                    } else {
                        bail!("basic type {:?} does not have a name?", tid);
                    }
                }
                TypeDetails::Enum(itid) => {
                    println!("\tenum {:?} {:?}", te, itid);
                    Ok(format!("Enum<{}>", "thing"))
                }
                TypeDetails::Array(itid) => Ok(format!("Vec<{}>", self.render_type(itid, in_mod)?)),
                TypeDetails::Optional(itid) => {
                    Ok(format!("Option<{}>", self.render_type(itid, in_mod)?))
                }
                TypeDetails::Object(_) => {
                    if let Some(n) = &te.name {
                        if in_mod {
                            Ok(n.to_string())
                        } else {
                            /*
                             * Model types are declared in the "types" module,
                             * and must be referenced with that prefix when not
                             * in the module itself.
                             */
                            Ok(format!("types::{}", n.to_string()))
                        }
                    } else {
                        bail!("object type {:?} does not have a name?", tid);
                    }
                }
                TypeDetails::Unknown => {
                    bail!("type {:?} is unknown", tid);
                }
            }
        } else {
            bail!("could not resolve type ID {:?}", tid);
        }
    }

    fn assign(&mut self) -> TypeId {
        let id = TypeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn id_for_name(&mut self, name: &str) -> TypeId {
        let id = if let Some(id) = self.name_to_id.get(name) {
            id.clone()
        } else {
            let id = self.assign();
            self.name_to_id.insert(name.to_string(), id.clone());
            id
        };
        id
    }

    fn id_for_optional(&mut self, want: &TypeId) -> TypeId {
        for (oid, oent) in self.id_to_entry.iter() {
            match &oent.details {
                TypeDetails::Optional(id) if id == want => return oid.clone(),
                _ => continue,
            }
        }

        let oid = self.assign();
        self.id_to_entry.insert(
            oid.clone(),
            TypeEntry {
                id: oid.clone(),
                name: None,
                details: TypeDetails::Optional(want.clone()),
            },
        );
        oid
    }

    fn prepop_reference(&mut self, name: &str, r: &str) -> Result<()> {
        let id = self.id_for_name(name);
        if let Some(rid) = self.ref_to_id.get(r) {
            println!("ref {:?}, name, {:?}, id {:?}, rid: {:?}", r, name, id, rid);
            if rid != &id {
                bail!(
                    "duplicate ref {:?}, name, {:?}, id {:?}, rid {:?}",
                    r,
                    name,
                    id,
                    rid
                );
            }
        } else {
            println!("ref {:?}, name, {:?}, id {:?}", r, name, id);
            self.ref_to_id.insert(r.to_string(), id);
        }
        Ok(())
    }

    fn select_ref(&mut self, _name: Option<&str>, r: &str) -> Result<TypeId> {
        /*
         * As this is a reference, all we can do for now is determine
         * the type ID.
         */
        Ok(if let Some(id) = self.ref_to_id.get(r) {
            id.clone()
        } else {
            let id = self.assign();
            self.ref_to_id.insert(r.to_string(), id.clone());
            id
        })
    }

    fn select_schema(
        &mut self,
        name: Option<&str>,
        s: &openapiv3::Schema,
        parent_name: &str,
    ) -> Result<TypeId> {
        let (name, details) = match &s.schema_kind {
            openapiv3::SchemaKind::Type(t) => match t {
                openapiv3::Type::Array(at) => {
                    /*
                     * Determine the type of item that will be in this array:
                     */
                    let itid = self.select_box(name, &at.items, parent_name)?;
                    (None, TypeDetails::Array(itid))
                }
                openapiv3::Type::Object(o) => {
                    /*
                     * Object types must have a consistent name.
                     */
                    let name = match (name, s.schema_data.title.as_deref()) {
                        (Some(n), None) => n.to_string(),
                        (Some(n), Some("")) => n.to_string(),
                        (None, Some(t)) => t.to_string(),
                        (Some(""), Some(t)) => t.to_string(),
                        (Some(n), Some(_)) => n.to_string(),
                        (None, None) => {
                            bail!("types need a name? {:?} {:?}", name, s)
                        }
                    };

                    let mut omap = BTreeMap::new();
                    for (n, rb) in o.properties.iter() {
                        let itid = self.select_box(
                            Some(n),
                            &rb,
                            &format!("{} {} {}", parent_name, name, n),
                        )?;
                        if o.required.contains(n) {
                            omap.insert(n.to_string(), itid);
                        } else {
                            /*
                             * This is an optional member.
                             */
                            omap.insert(n.to_string(), self.id_for_optional(&itid));
                        }
                    }
                    (Some(name), TypeDetails::Object(omap))
                }
                openapiv3::Type::String(st) => {
                    use openapiv3::{
                        StringFormat::Date,
                        StringFormat::DateTime,
                        VariantOrUnknownOrEmpty::{Empty, Item, Unknown},
                    };

                    match &st.format {
                        Item(DateTime) => {
                            self.import_chrono = true;
                            (Some("DateTime<Utc>".to_string()), TypeDetails::Basic)
                        }
                        Item(Date) => {
                            self.import_chrono = true;
                            (Some("NaiveDate".to_string()), TypeDetails::Basic)
                        }
                        Empty => (Some("String".to_string()), TypeDetails::Basic),
                        Unknown(f) => match f.as_str() {
                            "float" => (Some("f64".to_string()), TypeDetails::Basic),
                            "uri" => (Some("String".to_string()), TypeDetails::Basic),
                            "uri-template" => (Some("String".to_string()), TypeDetails::Basic),
                            "email" => (Some("String".to_string()), TypeDetails::Basic),
                            f => bail!("XXX unknown string format {}", f),
                        },
                        x => {
                            bail!("XXX string format {:?}", x);
                        }
                    }
                }
                openapiv3::Type::Boolean {} => (Some("bool".to_string()), TypeDetails::Basic),
                openapiv3::Type::Number(_) => {
                    /*
                     * XXX
                     */
                    (Some("f64".to_string()), TypeDetails::Basic)
                }
                openapiv3::Type::Integer(_) => {
                    /*
                     * XXX
                     */
                    (Some("i64".to_string()), TypeDetails::Basic)
                }
            },
            openapiv3::SchemaKind::AllOf { all_of } => {
                // TODO: Actually combine all the types.
                let id = self.select(name, all_of.get(0).unwrap())?;
                if let Some(et) = self.id_to_entry.get(&id) {
                    if let Some(n) = name {
                        (Some(n.to_string()), et.details.clone())
                    } else {
                        bail!("all_of types need a name? {:?} {:?}", name, all_of)
                    }
                } else {
                    bail!("allof schema kind: {:?} {:?}\n{:?}", name, s, all_of);
                }
            }
            openapiv3::SchemaKind::OneOf { one_of } => {
                // Iterate over each one of an select the first one that is not
                // an empty object.
                let mut id = TypeId(0);
                for o in one_of {
                    if let Ok(i) = self.select(name, o) {
                        id = i;
                        break;
                    }
                }

                if let Some(et) = self.id_to_entry.get(&id) {
                    if let Some(n) = name {
                        (Some(n.to_string()), et.details.clone())
                    } else {
                        bail!("one_of types need a name? {:?} {:?}", name, one_of)
                    }
                } else {
                    bail!("oneof schema kind: {:?} {:?}\n{:?}", name, s, one_of);
                }
            }
            openapiv3::SchemaKind::AnyOf { any_of } => {
                // TODO: Actually combine all the types.
                let id = self.select(name, any_of.get(0).unwrap())?;
                if let Some(et) = self.id_to_entry.get(&id) {
                    if let Some(n) = name {
                        (Some(n.to_string()), et.details.clone())
                    } else {
                        bail!("any_of types need a name? {:?} {:?}", name, any_of)
                    }
                } else {
                    bail!("anyof schema kind: {:?} {:?}\n{:?}", name, s, any_of);
                }
            }
            openapiv3::SchemaKind::Any(a) => {
                // Then we use the serde_json type.
                println!("\t{:?} any schema kind: {:?}", name, a);
                (Some("serde_json::Value".to_string()), TypeDetails::Basic)
            }
        };

        if let Some(name) = &name {
            /*
             * First, determine what ID we will use to identify this named type.
             */
            let id = self.id_for_name(name.as_str());

            /*
             * If there is already an entry for this type ID, ensure that it
             * matches the entry we have constructed.  If there is not yet an
             * entry, we can just keep this one.
             */
            if let Some(et) = self.id_to_entry.get(&id) {
                if et.details != details {
                    // We can get here if there are two objects with the same name
                    // that have properties that are different.
                    // Let's check if we already have an object with the parent name.
                    let mut pn = parent_name.to_string();
                    if parent_name.is_empty() {
                        // Append "Data" to the name.
                        pn = format!("{} Data", name);
                    }
                    let parent_id = self.id_for_name(&pn);
                    if let Some(pet) = self.id_to_entry.get(&parent_id) {
                        // We already have an item with the parent name!
                        if pet.details != details {
                            // We can get here if there are two objects with the same name
                            // that have properties that are different.
                            // Let's rename the new object with the parent name.
                            bail!(
                                "object details for {} do not match: {:?} != {:?}",
                                pn,
                                pet.details,
                                details,
                            );
                        }
                    } else {
                        // Let's rename the new object with the parent name.
                        // Insert the new one with the parent name.
                        self.id_to_entry.insert(
                            parent_id.clone(),
                            TypeEntry {
                                id: parent_id.clone(),
                                name: Some(pn),
                                details,
                            },
                        );
                    }
                }
            } else {
                self.id_to_entry.insert(
                    id.clone(),
                    TypeEntry {
                        id: id.clone(),
                        name: Some(name.clone()),
                        details,
                    },
                );
            }

            Ok(id)
        } else {
            /*
             * If this type has no name, look for an existing unnamed type with
             * the same shape.
             */
            for (tid, te) in self.id_to_entry.iter() {
                if te.name.is_none() && te.details == details {
                    return Ok(tid.clone());
                }
            }

            /*
             * Otherwise, insert a new entry.
             */
            let tid = self.assign();
            self.id_to_entry.insert(
                tid.clone(),
                TypeEntry {
                    id: tid.clone(),
                    name: None,
                    details,
                },
            );
            Ok(tid)
        }
    }

    fn select(
        &mut self,
        name: Option<&str>,
        s: &openapiv3::ReferenceOr<openapiv3::Schema>,
    ) -> Result<TypeId> {
        match s {
            openapiv3::ReferenceOr::Reference { reference } => {
                self.select_ref(name, reference.as_str())
            }
            openapiv3::ReferenceOr::Item(s) => self.select_schema(name, s, ""),
        }
    }

    fn select_box(
        &mut self,
        name: Option<&str>,
        s: &openapiv3::ReferenceOr<Box<openapiv3::Schema>>,
        parent_name: &str,
    ) -> Result<TypeId> {
        match s {
            openapiv3::ReferenceOr::Reference { reference } => {
                self.select_ref(name, reference.as_str())
            }
            openapiv3::ReferenceOr::Item(s) => self.select_schema(name, s.as_ref(), parent_name),
        }
    }
}

fn gen(
    api: &OpenAPI,
    ts: &mut TypeSpace,
    parameters: BTreeMap<String, &openapiv3::Parameter>,
) -> Result<String> {
    let mut out = String::new();

    let mut a = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };

    /*
     * Deal with any dependencies we require to produce this client.
     */
    a("");
    a("use anyhow::Result;"); /* XXX */
    a("");

    a("mod progenitor_support {");
    a("    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};");
    a("");
    /*
     * The percent-encoding crate abrogates its responsibility for providing
     * useful percent-encoding sets, so we must provide one for path components
     * here.
     */
    a("    const PATH_SET: &AsciiSet = &CONTROLS");
    /*
     * The query percent-encode set is the C0 control percent-encode set and
     * U+0020 SPACE, U+0022 ("), U+0023 (#), U+003C (<), and U+003E (>).
     */
    a("        .add(b' ')");
    a("        .add(b'\"')");
    a("        .add(b'#')");
    a("        .add(b'<')");
    a("        .add(b'>')");
    /*
     * The path percent-encode set is the query percent-encode set and U+003F
     * (?), U+0060 (`), U+007B ({), and U+007D (}).
     */
    a("        .add(b'?')");
    a("        .add(b'`')");
    a("        .add(b'{')");
    a("        .add(b'}');");
    a("");
    a("    pub(crate) fn encode_path(pc: &str) -> String {");
    a("        utf8_percent_encode(pc, PATH_SET).to_string()");
    a("    }");
    a("}");
    a("");

    /*
     * Declare named types we know about:
     */
    a("pub mod types {");
    if ts.import_chrono {
        a("    use chrono::prelude::*;");
    }
    a("    use serde::{Serialize, Deserialize};");
    a("");
    for te in ts.id_to_entry.values() {
        match &te.details {
            TypeDetails::Object(omap) => {
                a("    #[derive(Serialize, Deserialize, Debug)]");
                a(&format!(
                    "    pub struct {} {{",
                    te.name.as_deref().unwrap()
                ));
                for (name, tid) in omap.iter() {
                    if let Ok(rt) = ts.render_type(tid, true) {
                        a(&format!("        pub {}: {},", name, rt));
                    }
                    // TODO: do something when this fails.
                }
                a("    }");
                a("");
            }
            TypeDetails::Basic => {}
            TypeDetails::Unknown => {}
            TypeDetails::Enum(_) => {}
            TypeDetails::Array(_) => {}
            TypeDetails::Optional(_) => {}
        }
    }
    a("}");
    a("");

    /*
     * Declare the client object:
     */
    a("pub struct Client {");
    a("    baseurl: String,");
    a("    client: reqwest::Client,");
    a("}");
    a("");

    a("impl Client {");
    a("    pub fn new(baseurl: &str) -> Client {");
    a("        let dur = std::time::Duration::from_secs(15);");
    a("        let client = reqwest::ClientBuilder::new()");
    a("            .connect_timeout(dur)");
    a("            .timeout(dur)");
    a("            .build()");
    a("            .unwrap();");
    a("");
    a("        Client {");
    a("            baseurl: baseurl.to_string(),");
    a("            client,");
    a("        }");
    a("    }");
    a("");

    /*
     * Generate a function for each Operation.
     *
     * XXX We should probably be producing an intermediate object for each of
     * these, which can link in to the type space, instead of doing this inline
     * here.
     */
    for (pn, p) in api.paths.iter() {
        let op = p.item()?;

        let mut gen = |p: &str, m: &str, o: Option<&openapiv3::Operation>| -> Result<()> {
            let o = if let Some(o) = o {
                o
            } else {
                return Ok(());
            };

            let oid = o.operation_id.as_deref().unwrap();
            a("    /**");
            a(&format!("     * {}: {} {}", oid, m, p));
            a("     */");

            let mut bounds: Vec<String> = Vec::new();

            println!("generating {} {}", p, m);

            let (body_param, body_func) = if let Some(b) = &o.request_body {
                let b = b.item()?;
                if b.is_binary()? {
                    bounds.push("B: Into<reqwest::Body>".to_string());
                    (Some("B".to_string()), Some("body".to_string()))
                } else {
                    let (ct, mt) = b.content.first().unwrap();
                    if !mt.encoding.is_empty() {
                        bail!("media type encoding not empty: {:#?}", mt);
                    }

                    if ct == "application/json" {
                        if let Some(s) = &mt.schema {
                            let tid = match s {
                                openapiv3::ReferenceOr::Reference { reference } => {
                                    ts.select_ref(None, reference.as_str())?
                                }
                                openapiv3::ReferenceOr::Item(item) => {
                                    let object_name = format!(
                                        "{} Request",
                                        summary_to_object_name(m, &o.summary.as_ref().unwrap())
                                    );
                                    ts.select_schema(Some(&object_name), item, "")?
                                }
                            };
                            (
                                Some(format!("&{}", ts.render_type(&tid, false)?)),
                                Some("json".to_string()),
                            )
                        } else {
                            bail!("media type encoding, no schema: {:#?}", mt);
                        }
                    } else if ct == "text/plain" || ct == "*/*" {
                        if let Some(s) = &mt.schema {
                            let tid = ts.select(None, s)?;
                            let rt = ts.render_type(&tid, false)?;
                            if rt == "String" {
                                (Some("&str".to_string()), Some("text".to_string()))
                            } else {
                                (Some(rt), Some("body".to_string()))
                            }
                        } else {
                            bail!("media type encoding, no schema: {:#?}", mt);
                        }
                    } else {
                        bail!("unhandled request content type: {}", ct);
                    }
                }
            } else {
                (None, None)
            };

            if bounds.is_empty() {
                a(&format!("    pub async fn {}(", oid));
            } else {
                a(&format!("    pub async fn {}<{}>(", oid, bounds.join(", ")));
            }
            a("        &self,");

            for par in o.parameters.iter() {
                let item = match par {
                    openapiv3::ReferenceOr::Reference { reference } => {
                        // Get the parameter from our BTreeMap.
                        if let Some(param) =
                            parameters.get(&reference.replace("#/components/parameters/", ""))
                        {
                            param
                        } else {
                            bail!("could not find parameter with reference: {}", reference);
                        }
                    }
                    openapiv3::ReferenceOr::Item(item) => item,
                };

                match item {
                    openapiv3::Parameter::Path {
                        parameter_data,
                        style: openapiv3::PathStyle::Simple,
                    } => {
                        /*
                         * XXX Parameter types should probably go through
                         * the type space...
                         */
                        let nam = &parameter_data.name;
                        let typ = parameter_data.render_type()?;
                        a(&format!("        {}: {},", nam, typ));
                    }
                    openapiv3::Parameter::Query {
                        parameter_data,
                        allow_reserved: _,
                        style: openapiv3::QueryStyle::Form,
                        allow_empty_value,
                    } => {
                        if let Some(aev) = allow_empty_value {
                            if *aev {
                                bail!("allow empty value is a no go");
                            }
                        }

                        /*
                         * XXX Parameter types should probably go through
                         * the type space...
                         */
                        let nam = &parameter_data.name;
                        let typ = parameter_data.render_type()?;
                        a(&format!("        {}: {},", nam, typ));
                    }
                    x => bail!("unhandled parameter type: {:#?}", x),
                }
            }

            if let Some(bp) = &body_param {
                a(&format!("        body: {},", bp));
            }

            // Only do the first.
            if let Some(only) = o.responses.responses.first() {
                match only.0 {
                    openapiv3::StatusCode::Code(n) => {
                        // 302 is the code returned from /orgs/{org}/migrations/{migration_id}/archive GET
                        if *n < 200 || *n > 303 {
                            bail!("code? {:#?}", only);
                        }
                    }
                    _ => bail!("code? {:#?}", only),
                }

                let i = only.1.item()?;
                if !i.headers.is_empty() {
                    // TODO: do response headers.
                    println!("no response headers for now");
                }

                if !i.links.is_empty() {
                    // TODO: do response links
                    println!("no response links for now");
                }
                /*
                 * XXX ignoring extensions.
                 */

                /*
                 * Look at the response content.  For now, support a single
                 * JSON-formatted response.
                 */
                if i.content.is_empty() {
                    a("    ) -> Result<()> {");
                } else {
                    match i.content.get("application/json") {
                        Some(mt) => {
                            if !mt.encoding.is_empty() {
                                bail!("media type encoding not empty: {:#?}", mt);
                            }

                            if let Some(s) = &mt.schema {
                                let tid = match s {
                                    openapiv3::ReferenceOr::Reference { reference } => {
                                        ts.select_ref(None, reference.as_str())?
                                    }
                                    openapiv3::ReferenceOr::Item(item) => {
                                        if let openapiv3::StatusCode::Code(c) = only.0 {
                                            let status_code = StatusCode::from_u16(*c).unwrap();
                                            let object_name = format!(
                                                "{} {} Response",
                                                summary_to_object_name(
                                                    m,
                                                    &o.summary.as_ref().unwrap()
                                                ),
                                                status_code
                                                    .canonical_reason()
                                                    .unwrap()
                                                    .to_lowercase()
                                            );
                                            ts.select_schema(Some(&object_name), item, "")?
                                        } else {
                                            bail!("got a range and not a code for {:?}", only.0);
                                        }
                                    }
                                };
                                a(&format!(
                                    "    ) -> Result<{}> {{",
                                    ts.render_type(&tid, false)?
                                ));
                            } else {
                                bail!("media type encoding, no schema: {:#?}", mt);
                            }
                        }
                        None => {
                            let (ct, mt) = i.content.first().unwrap();
                            if ct == "text/plain"
                                || ct == "text/html"
                                || ct == "application/octocat-stream"
                                || ct == "*/*"
                            {
                                if let Some(s) = &mt.schema {
                                    let tid = ts.select(None, s)?;
                                    let rt = ts.render_type(&tid, false)?;
                                    println!("ct {} render_type {}", ct, rt);

                                    a(&format!("    ) -> Result<{}> {{", rt));
                                } else {
                                    bail!("media type encoding, no schema: {:#?}", mt);
                                }
                            } else {
                                bail!("unhandled response content type: {}", ct);
                            }
                        }
                    }
                }
            } else {
                bail!("responses? {:#?}", o.responses);
            }

            /*
             * Generate the URL for the request.
             */
            let tmp = template::parse(p)?;
            a(&tmp.compile());

            /*
             * Perform the request.
             */
            a(&format!(
                "        let res = self.client.{}(url)",
                m.to_lowercase()
            ));
            if let Some(f) = &body_func {
                a(&format!("            .{}(body)", f));
            }
            a("            .send()");
            a("            .await?");
            a("            .error_for_status()?;"); /* XXX */

            a("");

            a("        Ok(res.json().await?)");
            a("    }");
            a("");

            Ok(())
        };

        gen(pn.as_str(), "GET", op.get.as_ref())?;
        gen(pn.as_str(), "PUT", op.put.as_ref())?;
        gen(pn.as_str(), "POST", op.post.as_ref())?;
        gen(pn.as_str(), "DELETE", op.delete.as_ref())?;
        gen(pn.as_str(), "OPTIONS", op.options.as_ref())?;
        gen(pn.as_str(), "HEAD", op.head.as_ref())?;
        gen(pn.as_str(), "PATCH", op.patch.as_ref())?;
        gen(pn.as_str(), "TRACE", op.trace.as_ref())?;
    }

    a("}");

    Ok(out)
}

fn summary_to_object_name(m: &str, s: &str) -> String {
    format!(
        "{} {}",
        m.to_lowercase(),
        &s.to_lowercase()
            .replace('.', "")
            .replace(" an ", " ")
            .replace(" or ", " ")
            .replace(" for ", " ")
            .replace(" to ", " ")
            .replace(" your ", " ")
            .replace(" the ", " ")
            .replace("(beta)", "")
            .replace("(legacy)", "")
            .replace("-", " ")
            .replace(" a ", " ")
            .replace("'", "")
            .replace(" of ", " ")
            .replace("authenticated user", "")
    )
}

fn main() -> Result<()> {
    let mut opts = getopts::Options::new();
    opts.parsing_style(getopts::ParsingStyle::StopAtFirstFree);
    opts.reqopt(
        "i",
        "",
        "OpenAPI definition document (JSON | YAML)",
        "INPUT",
    );
    opts.reqopt("o", "", "Generated Rust crate directory", "OUTPUT");
    opts.reqopt("n", "", "Target Rust crate name", "CRATE");
    opts.reqopt("v", "", "Target Rust crate version", "VERSION");

    let args = match opts.parse(std::env::args().skip(1)) {
        Ok(args) => {
            if !args.free.is_empty() {
                eprintln!("{}", opts.usage("progenitor"));
                bail!("unexpected positional arguments");
            }
            args
        }
        Err(e) => {
            eprintln!("{}", opts.usage("progenitor"));
            bail!(e);
        }
    };

    let api = load_api(&args.opt_str("i").unwrap())?;

    let mut ts = TypeSpace::new();
    let mut parameters: BTreeMap<String, &openapiv3::Parameter> = BTreeMap::new();

    if let Some(components) = &api.components {
        /*
         * First, grant each expected reference a type ID.  Each
         * "components.schemas" entry needs an established reference for
         * resolution in this and other parts of the document.
         */
        for n in components.schemas.keys() {
            println!("PREPOP SCHEMA {}:", n);
            ts.prepop_reference(n, &format!("#/components/schemas/{}", n))?;
        }
        println!();

        /*
         * Populate a type to describe each entry in the schemas section:
         */
        for (i, (sn, s)) in components.schemas.iter().enumerate() {
            println!("SCHEMA {}/{}: {}", i + 1, components.schemas.len(), sn);

            let id = ts.select(Some(sn.as_str()), s)?;
            println!("    -> {:?}", id);

            println!();
        }

        println!();
        for (i, (pn, p)) in components.parameters.iter().enumerate() {
            println!(
                "PARAMETER {}/{}: {}",
                i + 1,
                components.parameters.len(),
                pn
            );

            match p {
                openapiv3::ReferenceOr::Reference { reference } => {
                    bail!("parameter {} uses reference, unsupported {}", pn, reference);
                }
                openapiv3::ReferenceOr::Item(item) => {
                    parameters.insert(pn.to_string(), item);
                }
            }
            println!();
        }
    }

    /*
     * In addition to types defined in schemas, types may be defined inline in
     * request and response bodies.
     */
    for (pn, p) in api.paths.iter() {
        let op = p.item()?;

        let grab = |pn: &str,
                    m: &str,
                    o: Option<&openapiv3::Operation>,
                    ts: &mut TypeSpace|
         -> Result<()> {
            if let Some(o) = o {
                /*
                 * Get the request body type, if this operation has one:
                 */
                if let Some(openapiv3::ReferenceOr::Item(body)) = &o.request_body {
                    if !body.is_binary()? {
                        if let Ok(mt) = body
                            .content_json()
                            .with_context(|| anyhow!("{} {} request", m, pn))
                        {
                            if let Some(s) = &mt.schema {
                                let object_name = format!(
                                    "{} Request",
                                    summary_to_object_name(m, &o.summary.as_ref().unwrap())
                                );
                                let id = ts.select(Some(&object_name), s)?;
                                println!("    {} {} request body -> {:?}", pn, m, id);
                            }
                        } else if let Some((ct, mt)) = body.content.first() {
                            if ct == "text/plain" || ct == "text/html" {
                                println!("    {} {} request body -> &str", pn, m,);
                            } else if let Some(s) = &mt.schema {
                                println!("    {} {} request body -> {:?}", pn, m, s);
                            } else {
                                bail!("unknown request content: {} {} {:?}", pn, m, body.content);
                            }
                        } else {
                            bail!("unknown request content: {} {} {:?}", pn, m, body.content);
                        }
                    }
                } else if let Some(openapiv3::ReferenceOr::Reference { reference }) =
                    &o.request_body
                {
                    let id = ts.select_ref(None, reference.as_str())?;
                    println!("    {} {} request body -> {:?}", pn, m, id);
                }

                /*
                 * Get the response body type for each status code:
                 */
                for (code, r) in o.responses.responses.iter() {
                    match r {
                        openapiv3::ReferenceOr::Item(ri) => {
                            if !ri.is_binary()? && !ri.content.is_empty() {
                                if let Ok(mt) = ri
                                    .content_json()
                                    .with_context(|| anyhow!("{} {} {}", m, pn, code))
                                {
                                    if let Some(s) = &mt.schema {
                                        if let openapiv3::StatusCode::Code(c) = code {
                                            let status_code = StatusCode::from_u16(*c).unwrap();
                                            let object_name = format!(
                                                "{} {} Response",
                                                summary_to_object_name(
                                                    m,
                                                    &o.summary.as_ref().unwrap()
                                                ),
                                                status_code
                                                    .canonical_reason()
                                                    .unwrap()
                                                    .to_lowercase()
                                            );
                                            let id = ts.select(Some(&object_name), s)?;
                                            println!(
                                                "    {} {} {} response body -> {:?}",
                                                pn, m, code, id
                                            );
                                        } else {
                                            bail!("got a range and not a code for {:?}", code);
                                        }
                                    }
                                } else if let Some((ct, mt)) = ri.content.first() {
                                    if ct == "text/plain" || ct == "text/html" {
                                        println!(
                                            "    {} {} {} response body -> String",
                                            pn, m, code,
                                        );
                                    } else if let Some(s) = &mt.schema {
                                        println!(
                                            "    {} {} {} response body -> {:?}",
                                            pn, m, code, s
                                        );
                                    } else {
                                        bail!(
                                            "unknown response content: {} {} {} {:?}",
                                            pn,
                                            m,
                                            code,
                                            ri.content
                                        );
                                    }
                                } else {
                                    bail!(
                                        "unknown response content: {} {} {} {:?}",
                                        pn,
                                        m,
                                        code,
                                        ri.content
                                    );
                                }
                            }
                        }
                        openapiv3::ReferenceOr::Reference { reference } => {
                            let id = ts.select_ref(None, reference.as_str())?;
                            println!("    {} {} {} response body -> {:?}", pn, m, code, id);
                        }
                    }
                }
            }
            Ok(())
        };

        grab(pn, "GET", op.get.as_ref(), &mut ts)?;
        grab(pn, "POST", op.post.as_ref(), &mut ts)?;
        grab(pn, "PUT", op.put.as_ref(), &mut ts)?;
        grab(pn, "DELETE", op.delete.as_ref(), &mut ts)?;
        grab(pn, "OPTIONS", op.options.as_ref(), &mut ts)?;
        grab(pn, "HEAD", op.head.as_ref(), &mut ts)?;
        grab(pn, "PATCH", op.patch.as_ref(), &mut ts)?;
        grab(pn, "TRACE", op.trace.as_ref(), &mut ts)?;
    }

    let fail = match gen(&api, &mut ts, parameters) {
        Ok(out) => {
            let name = args.opt_str("n").unwrap();
            let version = args.opt_str("v").unwrap();

            /*
             * Create the top-level crate directory:
             */
            let root = PathBuf::from(args.opt_str("o").unwrap());
            std::fs::create_dir_all(&root)?;

            /*
             * Write the Cargo.toml file:
             */
            let mut toml = root.clone();
            toml.push("Cargo.toml");
            let tomlout = format!(
                "[package]\n\
                name = \"{}\"\n\
                version = \"{}\"\n\
                edition = \"2018\"\n\
                \n\
                [dependencies]\n\
                anyhow = \"1\"\n\
                chrono = \"0.4\"\n\
                percent-encoding = \"2.1\"\n\
                reqwest = {{ version = \"0.11\", features = [\"json\"] }}\n\
                serde = {{ version = \"1\", features = [\"derive\"] }}\n",
                name, version,
            );
            save(&toml, tomlout.as_str())?;

            /*
             * Create the src/ directory:
             */
            let mut src = root;
            src.push("src");
            std::fs::create_dir_all(&src)?;

            /*
             * Create the Rust source file containing the generated client:
             */
            let mut librs = src;
            librs.push("lib.rs");
            save(librs, out.as_str())?;
            false
        }
        Err(e) => {
            println!("gen fail: {:?}", e);
            true
        }
    };

    println!("-----------------------------------------------------");
    println!(" TYPE SPACE");
    println!("-----------------------------------------------------");
    for te in ts.id_to_entry.values() {
        let n = ts.describe(&te.id);
        println!("{:>4}  {}", te.id.0, n);
    }
    println!("-----------------------------------------------------");
    println!();

    if fail {
        bail!("generation experienced errors");
    }

    Ok(())
}