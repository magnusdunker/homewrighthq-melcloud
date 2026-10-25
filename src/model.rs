//! MELCloud's device data and how it maps to entity states and commands.
//!
//! A command is the whole state from `Device/Get` with the changed fields,
//! `EffectiveFlags` telling which fields to apply, and `HasPendingCommand`.
//! The flag values are the ones pymelcloud uses.

use serde_json::{json, Map, Value};

pub const DEVICE_ATA: i64 = 0;
pub const DEVICE_ATW: i64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub id: i64,
    pub building_id: i64,
    pub name: String,
    pub device_type: i64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub mac: Option<String>,
    /// The `Device` object from `ListDevices`: what the unit supports, and
    /// values such as energy use that `Device/Get` does not have.
    pub conf: Map<String, Value>,
}

impl DeviceInfo {
    pub fn kind(&self) -> &'static str {
        match self.device_type {
            DEVICE_ATA => "Ata",
            DEVICE_ATW => "Atw",
            _ => "",
        }
    }
}

fn int(m: &Map<String, Value>, k: &str) -> Option<i64> {
    m.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

fn num(m: &Map<String, Value>, k: &str) -> Option<f64> {
    m.get(k).and_then(Value::as_f64)
}

fn flag(m: &Map<String, Value>, k: &str) -> Option<bool> {
    m.get(k).and_then(Value::as_bool)
}

/// Every device in a `User/ListDevices` answer. Devices sit directly under a
/// building's `Structure`, under its floors, under its areas, and under the
/// floors' areas.
pub fn parse_devices(v: &Value) -> Vec<DeviceInfo> {
    let mut out = vec![];
    for b in v.as_array().into_iter().flatten() {
        let s = &b["Structure"];
        let mut lists = vec![&s["Devices"]];
        for a in s["Areas"].as_array().into_iter().flatten() {
            lists.push(&a["Devices"]);
        }
        for f in s["Floors"].as_array().into_iter().flatten() {
            lists.push(&f["Devices"]);
            for a in f["Areas"].as_array().into_iter().flatten() {
                lists.push(&a["Devices"]);
            }
        }
        for d in lists.into_iter().filter_map(Value::as_array).flatten() {
            let Some(id) = d["DeviceID"].as_i64() else { continue };
            if out.iter().any(|x: &DeviceInfo| x.id == id) {
                continue;
            }
            let conf = d["Device"].as_object().cloned().unwrap_or_default();
            let device_type = int(&conf, "DeviceType").or_else(|| d["Type"].as_i64()).unwrap_or(-1);
            let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
            let model = conf
                .get("Units")
                .and_then(Value::as_array)
                .and_then(|u| u.iter().find(|u| u["IsIndoor"].as_bool().unwrap_or(true)).or(u.first()))
                .and_then(|u| text(&u["Model"]));
            out.push(DeviceInfo {
                id,
                building_id: d["BuildingID"].as_i64().or_else(|| b["ID"].as_i64()).unwrap_or(0),
                name: text(&d["DeviceName"]).unwrap_or_else(|| format!("MELCloud {id}")),
                device_type,
                model,
                serial: text(&d["SerialNumber"]),
                mac: text(&d["MacAddress"]),
                conf,
            });
        }
    }
    out
}

/// Whether the unit is reported offline.
pub fn offline(state: &Map<String, Value>) -> bool {
    flag(state, "Offline").unwrap_or(false)
}

/// The whole state with the changes, ready for `Device/SetAta` or `SetAtw`.
pub fn command(state: &Map<String, Value>, changes: &Map<String, Value>, flags: u64) -> Value {
    let mut s = state.clone();
    for (k, v) in changes {
        s.insert(k.clone(), v.clone());
    }
    s.insert("EffectiveFlags".into(), json!(flags));
    s.insert("HasPendingCommand".into(), json!(true));
    Value::Object(s)
}

/// A change: the fields to set and the flags that make MELCloud apply them.
pub type Change = (Map<String, Value>, u64);

fn change(fields: Value, flags: u64) -> Change {
    (fields.as_object().cloned().unwrap_or_default(), flags)
}

fn merge(a: Change, b: Change) -> Change {
    let mut m = a.0;
    m.extend(b.0);
    (m, a.1 | b.1)
}

pub fn temperature_sensor(state: &Map<String, Value>, conf: &Map<String, Value>, key: &str) -> Option<f64> {
    num(state, key).or_else(|| num(conf, key))
}

// ---------------------------------------------------------------------------
// air-to-air

pub mod ata_flags {
    pub const POWER: u64 = 0x1;
    pub const OPERATION_MODE: u64 = 0x2;
    pub const TEMPERATURE: u64 = 0x4;
    pub const FAN_SPEED: u64 = 0x8;
    pub const VANE_VERTICAL: u64 = 0x10;
    pub const VANE_HORIZONTAL: u64 = 0x100;
}

pub const ATA_MODES: [(i64, &str); 5] = [(1, "heat"), (2, "dry"), (3, "cool"), (7, "fan_only"), (8, "heat_cool")];
pub const VANE_VERTICAL: [(i64, &str); 7] = [(0, "auto"), (1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5"), (7, "swing")];
pub const VANE_HORIZONTAL: [(i64, &str); 8] = [(0, "auto"), (1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5"), (8, "split"), (12, "swing")];
/// target temperature, fan mode, turn off, turn on
pub const ATA_FEATURES: u64 = 1 | 8 | 128 | 256;

fn name_of(table: &[(i64, &'static str)], code: Option<i64>) -> Option<&'static str> {
    let code = code?;
    table.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

fn code_of(table: &[(i64, &str)], name: &str) -> Option<i64> {
    table.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
}

pub fn options(table: &[(i64, &str)]) -> Vec<String> {
    table.iter().map(|(_, n)| n.to_string()).collect()
}

pub fn ata_hvac_modes(conf: &Map<String, Value>) -> Vec<&'static str> {
    let mut v = vec!["off"];
    let has = |k: &str| flag(conf, k).unwrap_or(true);
    for (on, m) in [
        (has("CanHeat"), "heat"),
        (has("CanCool"), "cool"),
        (has("ModelSupportsDryMode"), "dry"),
        (true, "fan_only"),
        (has("ModelSupportsAuto"), "heat_cool"),
    ] {
        if on {
            v.push(m);
        }
    }
    v
}

pub fn fan_speeds(state: &Map<String, Value>, conf: &Map<String, Value>) -> i64 {
    if !flag(conf, "ModelSupportsFanSpeed").unwrap_or(true) {
        return 0;
    }
    int(state, "NumberOfFanSpeeds").or_else(|| int(conf, "NumberOfFanSpeeds")).unwrap_or(5).clamp(0, 9)
}

pub fn ata_fan_modes(state: &Map<String, Value>, conf: &Map<String, Value>) -> Vec<String> {
    let n = fan_speeds(state, conf);
    if n == 0 {
        return vec![];
    }
    let mut v = vec!["auto".to_string()];
    v.extend((1..=n).map(|i| i.to_string()));
    v
}

pub fn ata_mode(state: &Map<String, Value>) -> &'static str {
    if !flag(state, "Power").unwrap_or(false) {
        return "off";
    }
    name_of(&ATA_MODES, int(state, "OperationMode")).unwrap_or("heat_cool")
}

/// The temperature range MELCloud allows in a mode.
pub fn ata_range(conf: &Map<String, Value>, mode: &str) -> (f64, f64) {
    let (lo, hi, dlo) = match mode {
        "heat" => ("MinTempHeat", "MaxTempHeat", 10.0),
        "heat_cool" => ("MinTempAutomatic", "MaxTempAutomatic", 16.0),
        _ => ("MinTempCoolDry", "MaxTempCoolDry", 16.0),
    };
    (num(conf, lo).unwrap_or(dlo), num(conf, hi).unwrap_or(31.0))
}

pub fn ata_step(conf: &Map<String, Value>) -> f64 {
    num(conf, "TemperatureIncrement").filter(|s| *s > 0.0).unwrap_or(0.5)
}

pub fn ata_climate(state: &Map<String, Value>, conf: &Map<String, Value>) -> (String, Map<String, Value>) {
    let mode = ata_mode(state);
    let range_mode = if mode == "off" { name_of(&ATA_MODES, int(state, "OperationMode")).unwrap_or("heat") } else { mode };
    let (lo, hi) = ata_range(conf, range_mode);
    let mut a = Map::new();
    a.insert("hvac_modes".into(), json!(ata_hvac_modes(conf)));
    a.insert("hvac_action".into(), json!(match mode {
        "off" => "off",
        "heat" => "heating",
        "cool" => "cooling",
        "dry" => "drying",
        "fan_only" => "fan",
        _ => "idle",
    }));
    a.insert("current_temperature".into(), json!(num(state, "RoomTemperature")));
    a.insert("temperature".into(), json!(num(state, "SetTemperature")));
    a.insert("min_temp".into(), json!(lo));
    a.insert("max_temp".into(), json!(hi));
    a.insert("target_temp_step".into(), json!(ata_step(conf)));
    let fans = ata_fan_modes(state, conf);
    let mut features = ATA_FEATURES;
    if fans.is_empty() {
        features &= !8;
    } else {
        let speed = int(state, "SetFanSpeed").unwrap_or(0);
        a.insert("fan_mode".into(), json!(if speed == 0 { "auto".to_string() } else { speed.to_string() }));
        a.insert("fan_modes".into(), json!(fans));
    }
    a.insert("supported_features".into(), json!(features));
    (mode.to_string(), a)
}

fn ata_hvac(mode: &str) -> Result<Change, String> {
    if mode == "off" {
        return Ok(change(json!({ "Power": false }), ata_flags::POWER));
    }
    let code = code_of(&ATA_MODES, if mode == "auto" { "heat_cool" } else { mode }).ok_or_else(|| format!("ukendt tilstand: {mode}"))?;
    Ok(change(json!({ "Power": true, "OperationMode": code }), ata_flags::POWER | ata_flags::OPERATION_MODE))
}

/// The change for a climate service call. `Err("")` means not supported.
pub fn ata_change(service: &str, data: &Map<String, Value>, state: &Map<String, Value>, conf: &Map<String, Value>) -> Result<Change, String> {
    match service {
        "turn_on" => Ok(change(json!({ "Power": true }), ata_flags::POWER)),
        "turn_off" => Ok(change(json!({ "Power": false }), ata_flags::POWER)),
        "set_hvac_mode" => ata_hvac(data.get("hvac_mode").and_then(Value::as_str).ok_or("hvac_mode mangler")?),
        "set_temperature" => {
            let mut c = (Map::new(), 0);
            let mut mode = name_of(&ATA_MODES, int(state, "OperationMode")).unwrap_or("heat");
            if let Some(m) = data.get("hvac_mode").and_then(Value::as_str) {
                c = ata_hvac(m)?;
                if m != "off" {
                    mode = if m == "auto" { "heat_cool" } else { m };
                }
            }
            if let Some(t) = data.get("temperature").and_then(Value::as_f64) {
                let (lo, hi) = ata_range(conf, mode);
                let step = ata_step(conf);
                let t = ((t / step).round() * step).clamp(lo, hi);
                c = merge(c, change(json!({ "SetTemperature": t }), ata_flags::TEMPERATURE));
            }
            if c.1 == 0 {
                return Err("temperature mangler".into());
            }
            Ok(c)
        }
        "set_fan_mode" => {
            let m = data.get("fan_mode").and_then(Value::as_str).ok_or("fan_mode mangler")?;
            let n = fan_speeds(state, conf);
            let speed = if m == "auto" { 0 } else { m.parse::<i64>().ok().filter(|s| (1..=n).contains(s)).ok_or_else(|| format!("ukendt blæserhastighed: {m}"))? };
            Ok(change(json!({ "SetFanSpeed": speed }), ata_flags::FAN_SPEED))
        }
        _ => Err(String::new()),
    }
}

pub fn vane_vertical(state: &Map<String, Value>) -> Option<&'static str> {
    name_of(&VANE_VERTICAL, int(state, "VaneVertical"))
}

pub fn vane_horizontal(state: &Map<String, Value>) -> Option<&'static str> {
    name_of(&VANE_HORIZONTAL, int(state, "VaneHorizontal"))
}

pub fn set_vane_vertical(option: &str) -> Option<Change> {
    code_of(&VANE_VERTICAL, option).map(|c| change(json!({ "VaneVertical": c }), ata_flags::VANE_VERTICAL))
}

pub fn set_vane_horizontal(option: &str) -> Option<Change> {
    code_of(&VANE_HORIZONTAL, option).map(|c| change(json!({ "VaneHorizontal": c }), ata_flags::VANE_HORIZONTAL))
}

/// Energy use since the unit was installed, in kWh. MELCloud reports Wh.
pub fn energy_kwh(conf: &Map<String, Value>) -> Option<f64> {
    if !flag(conf, "HasEnergyConsumedMeter").unwrap_or(false) {
        return None;
    }
    num(conf, "CurrentEnergyConsumed").map(|wh| wh / 1000.0)
}

// ---------------------------------------------------------------------------
// air-to-water (Ecodan)

pub mod atw_flags {
    pub const POWER: u64 = 0x1;
    pub const ZONE1_MODE: u64 = 0x8;
    pub const ZONE2_MODE: u64 = 0x10;
    pub const FORCED_HOT_WATER: u64 = 0x10000;
    pub const TANK_TEMPERATURE: u64 = 0x1000000000020;
    pub const ZONE1_TEMPERATURE: u64 = 0x200000080;
    pub const ZONE2_TEMPERATURE: u64 = 0x800000200;
    pub const FLOW_TEMPERATURE: u64 = 0x1000000000000;
}

/// `OperationMode` of an air-to-water unit: what it is doing now.
pub const ATW_STATUS: [(i64, &str); 7] =
    [(0, "idle"), (1, "hot_water"), (2, "heating"), (3, "cooling"), (4, "defrost"), (5, "standby"), (6, "legionella")];
/// `OperationModeZone1/2`: heating to a room temperature, to a flow
/// temperature or by the heating curve, and cooling to a room or flow
/// temperature.
pub const ZONE_MODES: [(i64, &str, &str); 5] =
    [(0, "heat", "room"), (1, "heat", "flow"), (2, "heat", "curve"), (3, "cool", "room"), (4, "cool", "flow")];
pub const ZONE_PRESETS: [&str; 3] = ["room", "flow", "curve"];
pub const WATER_MODES: [&str; 2] = ["auto", "force_hot_water"];
/// target temperature, operation mode, on/off
pub const WATER_FEATURES: u64 = 1 | 2 | 8;
/// target temperature, preset, turn off, turn on
pub const ZONE_FEATURES: u64 = 1 | 16 | 128 | 256;
pub const TANK_MIN: f64 = 40.0;

pub fn atw_status(state: &Map<String, Value>) -> Option<&'static str> {
    name_of(&ATW_STATUS, int(state, "OperationMode"))
}

pub fn has_zone2(conf: &Map<String, Value>) -> bool {
    flag(conf, "HasZone2").unwrap_or(false)
}

fn power(state: &Map<String, Value>) -> bool {
    flag(state, "Power").unwrap_or(false)
}

pub fn tank_max(conf: &Map<String, Value>) -> f64 {
    num(conf, "MaxTankTemperature").filter(|t| *t > TANK_MIN).unwrap_or(60.0)
}

pub fn water_heater(state: &Map<String, Value>, conf: &Map<String, Value>) -> (String, Map<String, Value>) {
    let mode = if flag(state, "ForcedHotWaterMode").unwrap_or(false) { "force_hot_water" } else { "auto" };
    let mut a = Map::new();
    a.insert("current_temperature".into(), json!(temperature_sensor(state, conf, "TankWaterTemperature")));
    a.insert("temperature".into(), json!(num(state, "SetTankWaterTemperature")));
    a.insert("min_temp".into(), json!(TANK_MIN));
    a.insert("max_temp".into(), json!(tank_max(conf)));
    a.insert("target_temp_step".into(), json!(1.0));
    a.insert("operation_list".into(), json!(WATER_MODES));
    a.insert("operation_mode".into(), json!(mode));
    a.insert("supported_features".into(), json!(WATER_FEATURES));
    (if power(state) { mode } else { "off" }.to_string(), a)
}

pub fn water_change(service: &str, data: &Map<String, Value>, conf: &Map<String, Value>) -> Result<Change, String> {
    match service {
        "turn_on" => Ok(change(json!({ "Power": true }), atw_flags::POWER)),
        "turn_off" => Ok(change(json!({ "Power": false }), atw_flags::POWER)),
        "set_temperature" => {
            let t = data.get("temperature").and_then(Value::as_f64).ok_or("temperature mangler")?;
            Ok(change(json!({ "SetTankWaterTemperature": t.round().clamp(TANK_MIN, tank_max(conf)) }), atw_flags::TANK_TEMPERATURE))
        }
        "set_operation_mode" => {
            let m = data.get("operation_mode").and_then(Value::as_str).ok_or("operation_mode mangler")?;
            match m {
                "auto" => Ok(change(json!({ "ForcedHotWaterMode": false }), atw_flags::FORCED_HOT_WATER)),
                "force_hot_water" => Ok(change(json!({ "ForcedHotWaterMode": true }), atw_flags::FORCED_HOT_WATER)),
                "off" => Ok(change(json!({ "Power": false }), atw_flags::POWER)),
                _ => Err(format!("ukendt tilstand: {m}")),
            }
        }
        _ => Err(String::new()),
    }
}

fn zone_mode(state: &Map<String, Value>, zone: u8) -> (&'static str, &'static str) {
    let code = int(state, &format!("OperationModeZone{zone}")).unwrap_or(0);
    ZONE_MODES.iter().find(|(c, _, _)| *c == code).map(|(_, h, p)| (*h, *p)).unwrap_or(("heat", "room"))
}

fn zone_code(hvac: &str, preset: &str) -> Option<i64> {
    let preset = if hvac == "cool" && preset == "curve" { "room" } else { preset };
    ZONE_MODES.iter().find(|(_, h, p)| *h == hvac && *p == preset).map(|(c, _, _)| *c)
}

pub fn zone_hvac_modes(conf: &Map<String, Value>) -> Vec<&'static str> {
    let mut v = vec!["off", "heat"];
    if flag(conf, "CanCool").unwrap_or(false) {
        v.push("cool");
    }
    v
}

pub fn zone_climate(state: &Map<String, Value>, conf: &Map<String, Value>, zone: u8) -> (String, Map<String, Value>) {
    let (hvac, preset) = zone_mode(state, zone);
    let on = power(state);
    let idle = flag(state, &format!("IdleZone{zone}")).unwrap_or(false);
    let action = match (on, idle, atw_status(state)) {
        (false, _, _) => "off",
        (true, true, _) => "idle",
        (true, false, Some("heating")) => "heating",
        (true, false, Some("cooling")) => "cooling",
        _ => "idle",
    };
    let mut a = Map::new();
    a.insert("hvac_modes".into(), json!(zone_hvac_modes(conf)));
    a.insert("hvac_action".into(), json!(action));
    a.insert("current_temperature".into(), json!(num(state, &format!("RoomTemperatureZone{zone}"))));
    a.insert("temperature".into(), json!(num(state, &format!("SetTemperatureZone{zone}"))));
    a.insert("min_temp".into(), json!(10.0));
    a.insert("max_temp".into(), json!(30.0));
    a.insert("target_temp_step".into(), json!(0.5));
    a.insert("preset_modes".into(), json!(ZONE_PRESETS));
    a.insert("preset_mode".into(), json!(preset));
    a.insert("supported_features".into(), json!(ZONE_FEATURES));
    (if on { hvac } else { "off" }.to_string(), a)
}

fn zone_mode_flag(zone: u8) -> u64 {
    if zone == 2 { atw_flags::ZONE2_MODE } else { atw_flags::ZONE1_MODE }
}

pub fn zone_change(service: &str, data: &Map<String, Value>, state: &Map<String, Value>, zone: u8) -> Result<Change, String> {
    let (hvac, preset) = zone_mode(state, zone);
    let mode_key = format!("OperationModeZone{zone}");
    let set_mode = |h: &str, p: &str| -> Result<Change, String> {
        let code = zone_code(h, p).ok_or_else(|| format!("ukendt tilstand: {h}/{p}"))?;
        Ok(change(json!({ mode_key.clone(): code }), zone_mode_flag(zone)))
    };
    match service {
        "turn_on" => Ok(change(json!({ "Power": true }), atw_flags::POWER)),
        "turn_off" => Ok(change(json!({ "Power": false }), atw_flags::POWER)),
        "set_hvac_mode" => {
            let m = data.get("hvac_mode").and_then(Value::as_str).ok_or("hvac_mode mangler")?;
            match m {
                "off" => Ok(change(json!({ "Power": false }), atw_flags::POWER)),
                "heat" | "cool" => Ok(merge(change(json!({ "Power": true }), atw_flags::POWER), set_mode(m, preset)?)),
                _ => Err(format!("ukendt tilstand: {m}")),
            }
        }
        "set_preset_mode" => {
            let p = data.get("preset_mode").and_then(Value::as_str).ok_or("preset_mode mangler")?;
            if !ZONE_PRESETS.contains(&p) || (hvac == "cool" && p == "curve") {
                return Err(format!("ukendt forindstilling: {p}"));
            }
            set_mode(hvac, p)
        }
        "set_temperature" => {
            let t = data.get("temperature").and_then(Value::as_f64).ok_or("temperature mangler")?;
            let t = ((t * 2.0).round() / 2.0).clamp(10.0, 30.0);
            let f = if zone == 2 { atw_flags::ZONE2_TEMPERATURE } else { atw_flags::ZONE1_TEMPERATURE };
            Ok(change(json!({ format!("SetTemperatureZone{zone}"): t }), f))
        }
        _ => Err(String::new()),
    }
}

pub const FLOW_MIN: f64 = 25.0;
pub const FLOW_MAX: f64 = 60.0;

pub fn flow_target(state: &Map<String, Value>, zone: u8) -> Option<f64> {
    num(state, &format!("SetHeatFlowTemperatureZone{zone}"))
}

pub fn set_flow_target(zone: u8, value: f64) -> Change {
    change(json!({ format!("SetHeatFlowTemperatureZone{zone}"): value.round().clamp(FLOW_MIN, FLOW_MAX) }), atw_flags::FLOW_TEMPERATURE)
}

/// Zone names from the unit, or "Zone 1" and "Zone 2".
pub fn zone_name(conf: &Map<String, Value>, zone: u8) -> String {
    conf.get(&format!("Zone{zone}Name")).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| format!("Zone {zone}"))
}
