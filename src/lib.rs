//! Mitsubishi Electric MELCloud: air conditioners (air-to-air) and Ecodan
//! heat pumps (air-to-water) through MELCloud, as an external integration
//! for HomewrightHQ.
//!
//! The user logs in with the e-mail and password from the MELCloud app.
//! An air-to-air unit becomes a climate entity with vane selects and
//! temperature sensors. An air-to-water unit becomes a water heater for the
//! tank, a climate entity per zone, a flow temperature number per zone and
//! sensors. MELCloud blocks accounts that ask too often, so each unit is read
//! every five minutes at most, and a 429 answer pauses polling.

pub mod api;
pub mod model;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use homewright_plugin::{async_trait, json, Device, Entity, Entry, Error, Flow, FlowReply, Host, Integration, Map, Value};

use api::{ApiError, Client};
use model::DeviceInfo;

pub const DOMAIN: &str = "melcloud";
/// Default and allowed range for reading each unit, in minutes.
pub const DEFAULT_SCAN_MINUTES: u64 = 5;
const MIN_SCAN_MINUTES: u64 = 3;
const MAX_SCAN_MINUTES: u64 = 60;
/// How often the device list (energy, flow and return temperatures) is read.
const LIST_POLL: Duration = Duration::from_secs(3600);
/// After a command MELCloud may still report the old values for a while.
const SETTLE: Duration = Duration::from_secs(90);
/// The pause after a 429 without Retry-After, and the shortest pause.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(30 * 60);

/// Logging to HomewrightHQ's log through stderr.
#[doc(hidden)]
pub fn emit(level: &str, message: String) {
    homewright_plugin::log(level, message);
}

#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::emit("INFO", format!($($t)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::emit("WARN", format!($($t)*)) };
}

fn login_schema(email: &str) -> Value {
    json!({
        "type": "object",
        "required": ["email", "password"],
        "properties": {
            "email": { "type": "string", "title": "E-mail", "default": email, "description": "Den e-mail, du logger ind med i MELCloud-appen" },
            "password": { "type": "string", "format": "password", "title": "Adgangskode" }
        }
    })
}

fn options_schema(minutes: u64) -> Value {
    json!({
        "type": "object",
        "properties": {
            "scan_interval": {
                "type": "integer", "title": "Minutter mellem opdateringer", "default": minutes,
                "minimum": MIN_SCAN_MINUTES, "maximum": MAX_SCAN_MINUTES,
                "description": "MELCloud spærrer konti, der spørger for tit. Hver enhed hentes for sig."
            }
        }
    })
}

fn base_url(get: impl Fn(&str) -> Option<String>) -> String {
    get("base_url").unwrap_or_else(|| api::BASE_URL.to_string())
}

fn error_code(e: &ApiError) -> &'static str {
    match e {
        ApiError::Auth => "invalid_auth",
        ApiError::RateLimited(_) => "rate_limited",
        ApiError::Other(_) => "cannot_connect",
    }
}

/// Units this integration can show.
fn supported(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    devices.into_iter().filter(|d| !d.kind().is_empty()).collect()
}

pub fn scan_interval(entry: &Entry) -> Duration {
    let m = entry.get("scan_interval").and_then(Value::as_u64).unwrap_or(DEFAULT_SCAN_MINUTES).clamp(MIN_SCAN_MINUTES, MAX_SCAN_MINUTES);
    Duration::from_secs(m * 60)
}

#[derive(Default)]
pub struct MelCloud {
    accounts: Mutex<HashMap<String, Arc<Account>>>,
}

impl MelCloud {
    fn account(&self, entry_id: &str) -> Result<Arc<Account>, Error> {
        self.accounts.lock().unwrap().get(entry_id).cloned().ok_or_else(|| Error::unavailable("opsætningen kører ikke"))
    }
}

#[async_trait]
impl Integration for MelCloud {
    async fn config_flow(&self, _host: &Host, flow: Flow, step: &str, input: Option<Value>) -> Result<FlowReply, Error> {
        match (step, input) {
            ("user", None) => Ok(FlowReply::form("user", login_schema(""))),
            ("reauth", None) => {
                let email = flow.entry.as_ref().and_then(|e| e.get_str("email")).unwrap_or_default().to_string();
                Ok(FlowReply::form("user", login_schema(&email)))
            }
            ("user", Some(v)) => {
                let email = v["email"].as_str().unwrap_or_default().trim().to_string();
                let password = v["password"].as_str().unwrap_or_default().to_string();
                let base = base_url(|k| flow.data.get(k).and_then(Value::as_str).map(String::from));
                let client = Client::new(&base, &email, &password);
                let devices = match client.list_devices().await {
                    Ok(list) => supported(model::parse_devices(&list)),
                    Err(e) => {
                        info!("{e}");
                        return Ok(FlowReply::form_with_errors("user", login_schema(&email), json!({ "base": error_code(&e) })));
                    }
                };
                if flow.source == "reauth" {
                    let mut data = flow.entry.as_ref().and_then(|e| e.data.as_object().cloned()).unwrap_or_default();
                    data.insert("email".into(), json!(email));
                    data.insert("password".into(), json!(password));
                    return Ok(FlowReply::create("", Value::Object(data)));
                }
                if devices.is_empty() {
                    return Ok(FlowReply::abort("no_devices_found"));
                }
                let mut data = flow.data.clone();
                data.insert("email".into(), json!(email));
                data.insert("password".into(), json!(password));
                Ok(FlowReply::create(&format!("MELCloud ({email})"), Value::Object(data)).with_unique_id(email.to_lowercase()))
            }
            (s, _) => Err(Error::invalid(format!("ukendt trin {s}"))),
        }
    }

    async fn options_flow(&self, _host: &Host, flow: Flow, _step: &str, input: Option<Value>) -> Result<FlowReply, Error> {
        let current = flow.entry.as_ref().map(|e| scan_interval(e).as_secs() / 60).unwrap_or(DEFAULT_SCAN_MINUTES);
        match input {
            None => Ok(FlowReply::form("init", options_schema(current))),
            Some(v) => {
                let m = v["scan_interval"].as_u64().unwrap_or(current).clamp(MIN_SCAN_MINUTES, MAX_SCAN_MINUTES);
                Ok(FlowReply::create("", json!({ "scan_interval": m })))
            }
        }
    }

    async fn setup_entry(&self, host: &Host, entry: Entry) -> Result<(), Error> {
        let entry_id = entry.entry_id.clone();
        let email = entry.get_str("email").ok_or_else(|| Error::auth_failed("e-mail mangler"))?.to_string();
        let password = entry.get_str("password").unwrap_or_default().to_string();
        let client = Arc::new(Client::new(&base_url(|k| entry.get_str(k).map(String::from)), &email, &password));

        let devices = match client.list_devices().await {
            Ok(list) => supported(model::parse_devices(&list)),
            Err(ApiError::Auth) => return Err(Error::auth_failed("MELCloud afviste e-mail eller adgangskode")),
            Err(e) => return Err(Error::not_ready(e)),
        };
        let _ = host.delete_issue(&entry_id, "auth").await;

        let account = Arc::new(Account {
            entry_id: entry_id.clone(),
            host: host.clone(),
            client: client.clone(),
            units: Mutex::new(HashMap::new()),
            entities: Mutex::new(vec![]),
            task: Mutex::new(None),
            paused_until: Mutex::new(None),
        });
        for info in devices {
            let state = match client.device(info.id, info.building_id).await {
                Ok(s) => s.as_object().cloned().unwrap_or_default(),
                Err(e) => return Err(Error::not_ready(e)),
            };
            account.register(&info, &state).await?;
            let id = info.id;
            account.units.lock().unwrap().insert(id, Unit { available: !model::offline(&state), info, state, last_control: None });
            account.publish(id).await;
        }

        let interval = scan_interval(&entry);
        let acc = account.clone();
        let task = tokio::spawn(async move {
            let mut last_list = Instant::now();
            loop {
                tokio::time::sleep(interval).await;
                let list_due = last_list.elapsed() >= LIST_POLL;
                acc.poll(list_due).await;
                if list_due {
                    last_list = Instant::now();
                }
            }
        });
        *account.task.lock().unwrap() = Some(task);
        if let Some(old) = self.accounts.lock().unwrap().insert(entry_id, account) {
            old.stop();
        }
        Ok(())
    }

    async fn unload_entry(&self, _host: &Host, entry_id: &str) -> Result<(), Error> {
        if let Some(a) = self.accounts.lock().unwrap().remove(entry_id) {
            a.stop();
        }
        Ok(())
    }

    async fn entity_service(&self, _host: &Host, entry_id: &str, entity_id: &str, service: &str, data: Map<String, Value>) -> Result<(), Error> {
        self.account(entry_id)?.call_service(entity_id, service, &data).await
    }

    async fn diagnostics(&self, _host: &Host, entry: Entry) -> Value {
        let mut d = entry.data.clone();
        if let Value::Object(m) = &mut d {
            m.insert("password".into(), json!("**REDACTED**"));
        }
        let units: Vec<Value> = self
            .accounts
            .lock()
            .unwrap()
            .get(&entry.entry_id)
            .map(|a| {
                a.units
                    .lock()
                    .unwrap()
                    .values()
                    .map(|u| json!({ "type": u.info.device_type, "model": u.info.model, "available": u.available, "state": u.state }))
                    .collect()
            })
            .unwrap_or_default();
        json!({ "entry": d, "units": units })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    AtaClimate,
    VaneVertical,
    VaneHorizontal,
    RoomTemperature,
    OutdoorTemperature,
    Energy,
    Water,
    TankTemperature,
    Zone(u8),
    ZoneTemperature(u8),
    ZoneFlow(u8),
    FlowTemperature,
    ReturnTemperature,
    Status,
}

type StateUpdate = (String, Map<String, Value>);

struct Unit {
    info: DeviceInfo,
    /// The last answer from `Device/Get` or `Device/Set*`.
    state: Map<String, Value>,
    available: bool,
    last_control: Option<Instant>,
}

struct Account {
    entry_id: String,
    host: Host,
    client: Arc<Client>,
    units: Mutex<HashMap<i64, Unit>>,
    /// (entity_id, device id, kind)
    entities: Mutex<Vec<(String, i64, Kind)>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    paused_until: Mutex<Option<Instant>>,
}

fn num_state(v: Option<f64>) -> Option<StateUpdate> {
    v.map(|t| (t.to_string(), Map::new()))
}

impl Account {
    fn stop(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }

    async fn register(&self, info: &DeviceInfo, state: &Map<String, Value>) -> Result<(), Error> {
        let host = &self.host;
        let dev = host
            .register_device(
                &self.entry_id,
                Device {
                    identifiers: vec![(DOMAIN.into(), info.id.to_string())],
                    name: info.name.clone(),
                    manufacturer: Some("Mitsubishi Electric".into()),
                    model: info.model.clone(),
                    serial_number: info.serial.clone(),
                    ..Default::default()
                },
            )
            .await?;
        let id = info.id;
        let entity = |key: &str, name: Option<&str>, domain: &str| {
            let mut e = Entity::new(domain, if key.is_empty() { id.to_string() } else { format!("{id}-{key}") }).device(&dev);
            e.name = name.map(String::from);
            e
        };
        let temp = |e: Entity| e.device_class("temperature").unit("°C").state_class("measurement");
        let conf = &info.conf;
        let mut list: Vec<(Kind, Entity, bool)> = vec![];

        if info.device_type == model::DEVICE_ATA {
            list.push((
                Kind::AtaClimate,
                entity("", None, "climate").capabilities(json!({
                    "hvac_modes": model::ata_hvac_modes(conf), "fan_modes": model::ata_fan_modes(state, conf),
                    "target_temp_step": model::ata_step(conf), "supported_features": model::ATA_FEATURES })),
                true,
            ));
            list.push((Kind::RoomTemperature, temp(entity("room", Some("Rumtemperatur"), "sensor")), false));
            if model::temperature_sensor(state, conf, "OutdoorTemperature").is_some() {
                list.push((Kind::OutdoorTemperature, temp(entity("outdoor", Some("Udetemperatur"), "sensor")), false));
            }
            if conf.get("ModelSupportsVaneVertical").and_then(Value::as_bool).unwrap_or(true) {
                list.push((
                    Kind::VaneVertical,
                    entity("vane_vertical", Some("Lodret lamel"), "select").icon("mdi:arrow-up-down").capabilities(json!({ "options": model::options(&model::VANE_VERTICAL) })),
                    true,
                ));
            }
            if conf.get("ModelSupportsVaneHorizontal").and_then(Value::as_bool).unwrap_or(false) {
                list.push((
                    Kind::VaneHorizontal,
                    entity("vane_horizontal", Some("Vandret lamel"), "select").icon("mdi:arrow-left-right").capabilities(json!({ "options": model::options(&model::VANE_HORIZONTAL) })),
                    true,
                ));
            }
            if model::energy_kwh(conf).is_some() {
                list.push((Kind::Energy, entity("energy", Some("Energiforbrug"), "sensor").device_class("energy").unit("kWh").state_class("total_increasing"), false));
            }
        } else {
            list.push((
                Kind::Water,
                entity("tank", Some("Varmtvand"), "water_heater").capabilities(json!({
                    "operation_list": model::WATER_MODES, "min_temp": model::TANK_MIN, "max_temp": model::tank_max(conf),
                    "supported_features": model::WATER_FEATURES })),
                true,
            ));
            list.push((Kind::TankTemperature, temp(entity("tank_temperature", Some("Varmtvandstemperatur"), "sensor")), false));
            let zones: Vec<u8> = if model::has_zone2(conf) { vec![1, 2] } else { vec![1] };
            for z in zones {
                let zname = model::zone_name(conf, z);
                list.push((
                    Kind::Zone(z),
                    entity(&format!("zone{z}"), Some(&zname), "climate").capabilities(json!({
                        "hvac_modes": model::zone_hvac_modes(conf), "preset_modes": model::ZONE_PRESETS,
                        "supported_features": model::ZONE_FEATURES })),
                    true,
                ));
                list.push((Kind::ZoneTemperature(z), temp(entity(&format!("zone{z}_temperature"), Some(&format!("{zname} temperatur")), "sensor")), false));
                list.push((
                    Kind::ZoneFlow(z),
                    entity(&format!("zone{z}_flow_target"), Some(&format!("{zname} fremløb")), "number")
                        .device_class("temperature")
                        .unit("°C")
                        .icon("mdi:thermometer-water")
                        .category("config")
                        .capabilities(json!({ "min": model::FLOW_MIN, "max": model::FLOW_MAX, "step": 1.0, "mode": "box" })),
                    true,
                ));
            }
            list.push((Kind::OutdoorTemperature, temp(entity("outdoor", Some("Udetemperatur"), "sensor")), false));
            list.push((Kind::FlowTemperature, temp(entity("flow", Some("Fremløbstemperatur"), "sensor")), false));
            list.push((Kind::ReturnTemperature, temp(entity("return", Some("Returtemperatur"), "sensor")), false));
            let status: Vec<&str> = model::ATW_STATUS.iter().map(|(_, s)| *s).collect();
            list.push((
                Kind::Status,
                entity("status", Some("Status"), "sensor").device_class("enum").icon("mdi:heat-pump").capabilities(json!({ "options": status })),
                false,
            ));
        }

        for (kind, e, handler) in list {
            if let Some(entity_id) = host.add_entity(&self.entry_id, e, handler).await? {
                self.entities.lock().unwrap().push((entity_id, id, kind));
            }
        }
        Ok(())
    }

    /// Push states for every entity of a unit.
    async fn publish(&self, id: i64) {
        let (available, updates): (bool, Vec<(String, Option<StateUpdate>)>) = {
            let units = self.units.lock().unwrap();
            let Some(u) = units.get(&id) else { return };
            let (s, c) = (&u.state, &u.info.conf);
            let list = self
                .entities
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, d, _)| *d == id)
                .map(|(entity_id, _, kind)| {
                    let value = match kind {
                        Kind::AtaClimate => Some(model::ata_climate(s, c)),
                        Kind::VaneVertical => model::vane_vertical(s).map(|v| (v.to_string(), Map::new())),
                        Kind::VaneHorizontal => model::vane_horizontal(s).map(|v| (v.to_string(), Map::new())),
                        Kind::RoomTemperature => num_state(model::temperature_sensor(s, c, "RoomTemperature")),
                        Kind::OutdoorTemperature => num_state(model::temperature_sensor(s, c, "OutdoorTemperature")),
                        Kind::Energy => model::energy_kwh(c).map(|e| (format!("{e:.3}"), Map::new())),
                        Kind::Water => Some(model::water_heater(s, c)),
                        Kind::TankTemperature => num_state(model::temperature_sensor(s, c, "TankWaterTemperature")),
                        Kind::Zone(z) => Some(model::zone_climate(s, c, *z)),
                        Kind::ZoneTemperature(z) => num_state(model::temperature_sensor(s, c, &format!("RoomTemperatureZone{z}"))),
                        Kind::ZoneFlow(z) => {
                            let mut a = Map::new();
                            a.insert("min".into(), json!(model::FLOW_MIN));
                            a.insert("max".into(), json!(model::FLOW_MAX));
                            a.insert("step".into(), json!(1.0));
                            a.insert("mode".into(), json!("box"));
                            model::flow_target(s, *z).map(|t| (t.to_string(), a))
                        }
                        Kind::FlowTemperature => num_state(model::temperature_sensor(s, c, "FlowTemperature")),
                        Kind::ReturnTemperature => num_state(model::temperature_sensor(s, c, "ReturnTemperature")),
                        Kind::Status => model::atw_status(s).map(|v| (v.to_string(), Map::new())),
                    };
                    (entity_id.clone(), value)
                })
                .collect();
            (u.available, list)
        };
        for (entity_id, value) in updates {
            let res = match value {
                _ if !available => self.host.set_unavailable(&entity_id).await,
                Some((state, attrs)) => self.host.set_state(&entity_id, state, attrs).await,
                None => self.host.set_state(&entity_id, "unknown", Map::new()).await,
            };
            if let Err(e) = res {
                warn!("kunne ikke opdatere {entity_id}: {e}");
            }
        }
    }

    fn paused(&self) -> bool {
        self.paused_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
    }

    async fn poll(&self, list_due: bool) {
        if self.paused() {
            return;
        }
        if list_due {
            match self.client.list_devices().await {
                Ok(list) => {
                    let mut units = self.units.lock().unwrap();
                    for d in model::parse_devices(&list) {
                        if let Some(u) = units.get_mut(&d.id) {
                            u.info.conf = d.conf;
                        }
                    }
                }
                Err(e) => {
                    self.report(&e).await;
                    if self.paused() {
                        return;
                    }
                }
            }
        }
        let ids: Vec<(i64, i64)> = self.units.lock().unwrap().values().map(|u| (u.info.id, u.info.building_id)).collect();
        for (id, building) in ids {
            let settling = self.units.lock().unwrap().get(&id).and_then(|u| u.last_control).is_some_and(|t| t.elapsed() < SETTLE);
            if !settling {
                match self.client.device(id, building).await {
                    Ok(v) => {
                        let _ = self.host.delete_issue(&self.entry_id, "auth").await;
                        if let (Some(u), Some(s)) = (self.units.lock().unwrap().get_mut(&id), v.as_object()) {
                            u.available = !model::offline(s);
                            u.state = s.clone();
                        }
                    }
                    Err(e) => {
                        self.report(&e).await;
                        if let ApiError::RateLimited(_) = e {
                            return;
                        }
                        if let Some(u) = self.units.lock().unwrap().get_mut(&id) {
                            u.available = false;
                        }
                    }
                }
            }
            self.publish(id).await;
        }
    }

    async fn report(&self, e: &ApiError) {
        match e {
            ApiError::Auth => {
                let r = self
                    .host
                    .create_issue(
                        &self.entry_id,
                        "auth",
                        "error",
                        "MELCloud: login afvist",
                        "MELCloud afviste e-mail eller adgangskode. Godkend integrationen igen med de rigtige oplysninger.",
                    )
                    .await;
                if let Err(e) = r {
                    warn!("kunne ikke oprette problem: {e}");
                }
            }
            ApiError::RateLimited(after) => {
                let pause = after.unwrap_or(RATE_LIMIT_PAUSE).max(RATE_LIMIT_PAUSE);
                warn!("MELCloud svarede 429; venter {} minutter", pause.as_secs() / 60);
                *self.paused_until.lock().unwrap() = Some(Instant::now() + pause);
            }
            ApiError::Other(err) => warn!("{err}"),
        }
    }

    async fn call_service(&self, entity_id: &str, service: &str, data: &Map<String, Value>) -> Result<(), Error> {
        let (id, kind) = self
            .entities
            .lock()
            .unwrap()
            .iter()
            .find(|(e, _, _)| e == entity_id)
            .map(|(_, d, k)| (*d, *k))
            .ok_or_else(|| Error::new("not_found", entity_id))?;
        let (state, conf, unit_kind) = {
            let units = self.units.lock().unwrap();
            let u = units.get(&id).ok_or_else(|| Error::unavailable(entity_id))?;
            (u.state.clone(), u.info.conf.clone(), u.info.kind())
        };
        let to_err = |m: String| if m.is_empty() { Error::not_supported(service) } else { Error::invalid(m) };
        let option = || data.get("option").and_then(Value::as_str).unwrap_or_default().to_string();
        let change = match (kind, service) {
            (Kind::AtaClimate, _) => model::ata_change(service, data, &state, &conf).map_err(to_err)?,
            (Kind::VaneVertical, "select_option") => model::set_vane_vertical(&option()).ok_or_else(|| Error::invalid(format!("ukendt stilling: {}", option())))?,
            (Kind::VaneHorizontal, "select_option") => model::set_vane_horizontal(&option()).ok_or_else(|| Error::invalid(format!("ukendt stilling: {}", option())))?,
            (Kind::Water, _) => model::water_change(service, data, &conf).map_err(to_err)?,
            (Kind::Zone(z), _) => model::zone_change(service, data, &state, z).map_err(to_err)?,
            (Kind::ZoneFlow(z), "set_value") => {
                let v = data.get("value").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).ok_or_else(|| Error::invalid("value mangler"))?;
                model::set_flow_target(z, v)
            }
            _ => return Err(Error::not_supported(service)),
        };
        if self.paused() {
            return Err(Error::unavailable("MELCloud har bedt om en pause efter for mange forespørgsler"));
        }
        let body = model::command(&state, &change.0, change.1);
        let answer = match self.client.set(unit_kind, &body).await {
            Ok(v) => v,
            Err(e) => {
                self.report(&e).await;
                return Err(Error::failed(e));
            }
        };
        if let Some(u) = self.units.lock().unwrap().get_mut(&id) {
            // MELCloud answers with the state it sends to the unit; fall back
            // to what was sent when the answer is empty
            let mut next = body.as_object().cloned().unwrap_or_default();
            if let Some(a) = answer.as_object() {
                next.extend(a.clone());
            }
            u.state = next;
            u.last_control = Some(Instant::now());
            u.available = true;
        }
        self.publish(id).await;
        Ok(())
    }
}
