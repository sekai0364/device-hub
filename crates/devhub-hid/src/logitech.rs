//! HID++ 2.0 long reports on the dedicated Logicool configuration collection.
//! Feature indices are looked up for each connection because they vary by firmware.

use crate::{transport::Channel, Watcher};
use devhub_core::*;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const REPORT: u8 = 0x11;
const MODE: &str = "logitech_profile_mode";
const DPI: &str = "logitech_dpi";
const RATE: &str = "logitech_report_rate";
const HOST: i64 = 2;
const ONBOARD: i64 = 1;
const STARTUP_INTERVAL_MS: u8 = 4; // 250 Hz
const HOST_REQUIRED: &str = "変更するにはプロファイルモードをホストに切り替えてください";
const RESPONSE_TIMEOUT: &str = "HID++ 応答タイムアウト";

struct Link {
    channel: Channel,
    slot: u8,
    software_id: u8,
}

impl Link {
    fn call(&mut self, feature: u8, function: u8, data: &[u8]) -> HubResult<[u8; 16]> {
        self.software_id = self.software_id % 15 + 1;
        let address = function << 4 | self.software_id;
        let mut request = [0; 19];
        request[..3].copy_from_slice(&[self.slot, feature, address]);
        request[3..3 + data.len()].copy_from_slice(data);
        self.channel.send_report(REPORT, &request)?;

        let deadline = Instant::now() + Duration::from_millis(1000);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let frame = match self.channel.recv(remaining.as_millis().max(1) as i32) {
                Err(HubError::Protocol(_)) => break,
                other => other?,
            };
            if let Some(payload) = reply(&frame, self.slot, feature, address)? {
                return Ok(payload);
            }
        }
        // A sleeping wireless mouse can stop responding while its receiver is
        // still connected. Reuse the HID handle when the mouse wakes.
        Err(HubError::Protocol(RESPONSE_TIMEOUT.into()))
    }

    fn set_mode(&mut self, feature: u8, mode: u8) -> HubResult<()> {
        self.call(feature, 1, &[mode])?;
        if self.call(feature, 2, &[])?[0] != mode {
            return Err(HubError::Protocol(
                "HID++ プロファイルモードが反映されませんでした".into(),
            ));
        }
        Ok(())
    }

    fn set_report_interval(&mut self, feature: u8, interval: u8) -> HubResult<()> {
        self.call(feature, 2, &[interval])?;
        if self.call(feature, 1, &[])?[0] != interval {
            return Err(HubError::Protocol(
                "HID++ ポーリングレートが反映されませんでした".into(),
            ));
        }
        Ok(())
    }

    fn feature(&mut self, id: u16) -> HubResult<u8> {
        Ok(self.call(0, 0, &id.to_be_bytes())?[0])
    }

    fn required_feature(&mut self, id: u16) -> HubResult<u8> {
        match self.feature(id)? {
            0 => Err(HubError::Protocol(format!(
                "HID++ feature {id:04X} is unavailable"
            ))),
            index => Ok(index),
        }
    }
}

fn reply(frame: &[u8], slot: u8, feature: u8, address: u8) -> HubResult<Option<[u8; 16]>> {
    match frame {
        [REPORT, device, 0xff, index, command, error, ..]
        | [0x10, device, 0x8f, index, command, error, ..]
            if (*device, *index, *command) == (slot, feature, address) =>
        {
            Err(HubError::Protocol(format!("HID++ error 0x{error:02X}")))
        }
        [REPORT, device, index, command, payload @ ..]
            if (*device, *index, *command) == (slot, feature, address) =>
        {
            let data = payload
                .get(..16)
                .ok_or_else(|| HubError::Protocol("HID++ long report is truncated".into()))?;
            let mut result = [0; 16];
            result.copy_from_slice(data);
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

#[derive(Clone, Copy)]
struct DpiRange {
    min: u16,
    step: u16,
    max: u16,
}

impl DpiRange {
    fn parse(data: [u8; 16]) -> HubResult<Self> {
        let min = u16::from_be_bytes([data[1], data[2]]);
        let marker = u16::from_be_bytes([data[3], data[4]]);
        let step = marker & 0x1fff;
        let max = u16::from_be_bytes([data[5], data[6]]);
        if data[0] != 0
            || min == 0
            || marker & 0xe000 != 0xe000
            || step == 0
            || max < min
            || (max - min) % step != 0
            || data[7..9] != [0, 0]
        {
            return Err(HubError::Protocol("Unsupported HID++ DPI range".into()));
        }
        Ok(Self { min, step, max })
    }

    fn contains(self, dpi: u16) -> bool {
        (self.min..=self.max).contains(&dpi) && (dpi - self.min) % self.step == 0
    }
}

pub(super) struct Logitech {
    info: DeviceInfo,
    link: Mutex<Link>,
    mode: u8,
    dpi: u8,
    dpi_range: DpiRange,
    rate: u8,
    rate_options: Vec<u8>,
    battery: u8,
    voltage_battery: bool,
    watcher: Option<Watcher>,
}

pub(super) fn open(
    device: hidapi::HidDevice,
    info: DeviceInfo,
    watcher: Option<Watcher>,
) -> HubResult<Logitech> {
    let mut link = Link {
        channel: Channel::open(device, 64, 20),
        slot: if info.transport == Transport::Wired {
            0xff
        } else {
            1
        },
        software_id: 9,
    };
    let identity = link.required_feature(0x0003)?;
    let ids = link.call(identity, 0, &[])?;
    let expected = match info.pid {
        0xc547 | 0xc094 => [0x40, 0x93, 0xc0, 0x94],
        0xc539 | 0xc088 => [0x40, 0x79, 0xc0, 0x88],
        _ => return Err(HubError::Unsupported("Logicool HID++ model")),
    };
    if ids[6] & 0x0f != 0x0c || ids[7..11] != expected {
        return Err(HubError::Unsupported("ペアリングされたマウスの機種"));
    }

    let mode = link.required_feature(0x8100)?;
    let dpi = link.required_feature(0x2201)?;
    let dpi_range = DpiRange::parse(link.call(dpi, 1, &[0])?)?;
    let rate = link.required_feature(0x8060)?;
    let mask = link.call(rate, 0, &[])?[0];
    let rate_options: Vec<u8> = (1..=8).filter(|ms| mask & (1 << (ms - 1)) != 0).collect();
    let unified = link.feature(0x1004)?;
    let battery = if unified == 0 {
        link.feature(0x1001)?
    } else {
        unified
    };
    // Apply the startup settings once per connection, before the first UI read.
    if rate_options.contains(&STARTUP_INTERVAL_MS) {
        if link.call(mode, 2, &[])?[0] != HOST as u8 {
            link.set_mode(mode, HOST as u8)?;
        }
        if link.call(rate, 1, &[])?[0] != STARTUP_INTERVAL_MS {
            link.set_report_interval(rate, STARTUP_INTERVAL_MS)?;
        }
    }
    Ok(Logitech {
        info,
        link: Mutex::new(link),
        mode,
        dpi,
        dpi_range,
        rate,
        rate_options,
        battery,
        voltage_battery: unified == 0,
        watcher,
    })
}

impl DeviceDriver for Logitech {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn caps(&self) -> Capabilities {
        let range = self.dpi_range;
        let mode = SettingSpec::choice(
            MODE,
            section::PERFORMANCE,
            "プロファイルモード",
            vec![
                ChoiceOpt::new(ONBOARD, "オンボード"),
                ChoiceOpt::new(HOST, "ホスト"),
            ],
        )
        .help("ホストモードではオンボードプロファイルの設定を一時的に使用しません。");
        let dpi = SettingSpec::slider(
            DPI,
            section::SENSOR,
            "DPI",
            range.min.into(),
            range.max.into(),
            range.step.into(),
            "DPI",
        )
        .enabled_when(MODE, SettingValue::Choice(HOST), HOST_REQUIRED);
        let rate = SettingSpec::choice(
            RATE,
            section::PERFORMANCE,
            "ポーリングレート",
            self.rate_options
                .iter()
                .map(|ms| {
                    let hz = 1000 / i64::from(*ms);
                    ChoiceOpt::new(hz, format!("{hz} Hz"))
                })
                .collect(),
        )
        .enabled_when(MODE, SettingValue::Choice(HOST), HOST_REQUIRED);
        Capabilities {
            settings: vec![mode, dpi, rate],
            battery: self.battery != 0,
            ..Capabilities::empty()
        }
    }

    fn read(&self) -> HubResult<DeviceState> {
        let mut link = self
            .link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mode = link.call(self.mode, 2, &[])?[0];
        let dpi = link.call(self.dpi, 2, &[0])?;
        let interval = link.call(self.rate, 1, &[])?[0];
        let mut settings = [
            (MODE.into(), SettingValue::Choice(mode.into())),
            (
                DPI.into(),
                SettingValue::Num(u16::from_be_bytes([dpi[1], dpi[2]]).into()),
            ),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
        if interval != 0 {
            settings.insert(
                RATE.into(),
                SettingValue::Choice(1000 / i64::from(interval)),
            );
        }
        Ok(DeviceState {
            profile: ProfileState::default(),
            settings,
            mouse: None,
            keyboard: None,
            lighting: Default::default(),
            peq: None,
        })
    }

    fn apply(&self, change: &Change) -> HubResult<()> {
        let Change::Setting { id, value } = change else {
            return Err(HubError::Unsupported("Logicool HID++ setting"));
        };
        let mut link = self
            .link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if id == MODE {
            let SettingValue::Choice(mode @ (ONBOARD | HOST)) = *value else {
                return Err(HubError::Invalid("HID++ profile mode".into()));
            };
            link.set_mode(self.mode, mode as u8)?;
            drop(link);
            if let Some(watcher) = &self.watcher {
                watcher(HubEvent::StateChanged(self.info.id.clone()));
            }
            return Ok(());
        }
        if link.call(self.mode, 2, &[])?[0] != HOST as u8 {
            return Err(HubError::Invalid(HOST_REQUIRED.into()));
        }
        match (id.as_str(), value) {
            (DPI, SettingValue::Num(dpi))
                if f32::from(*dpi as u16) == *dpi && self.dpi_range.contains(*dpi as u16) =>
            {
                let [hi, lo] = (*dpi as u16).to_be_bytes();
                link.call(self.dpi, 3, &[0, hi, lo])?;
                if link.call(self.dpi, 2, &[0])?[1..3] != [hi, lo] {
                    return Err(HubError::Protocol(
                        "HID++ DPI が反映されませんでした".into(),
                    ));
                }
            }
            (RATE, SettingValue::Choice(hz)) => {
                let ms = self
                    .rate_options
                    .iter()
                    .copied()
                    .find(|ms| 1000 / i64::from(*ms) == *hz)
                    .ok_or_else(|| HubError::Invalid(format!("{hz} Hz")))?;
                link.set_report_interval(self.rate, ms)?;
            }
            _ => return Err(HubError::Invalid(id.clone())),
        }
        Ok(())
    }

    fn battery(&self) -> HubResult<Battery> {
        if self.battery == 0 {
            return Err(HubError::Unsupported("battery"));
        }
        let mut link = self
            .link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.voltage_battery {
            let data = link.call(self.battery, 0, &[])?;
            return Ok(Battery {
                percent: None,
                charging: data[2] & 0x80 != 0,
            });
        }
        let data = link.call(self.battery, 1, &[])?;
        Ok(Battery {
            percent: (data[0] <= 100).then_some(data[0]),
            charging: matches!(data[2], 1 | 2),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_responses_match_device_and_request() {
        let mut frame = [0; 20];
        frame[..6].copy_from_slice(&[REPORT, 1, 10, 0x2b, 3, 0x20]);
        assert_eq!(reply(&frame, 2, 10, 0x2b).unwrap(), None);
        assert_eq!(reply(&frame, 1, 10, 0x2a).unwrap(), None);
        assert_eq!(reply(&frame, 1, 10, 0x2b).unwrap().unwrap()[..2], [3, 0x20]);
        assert!(reply(&frame[..7], 1, 10, 0x2b).is_err());
        assert!(reply(&[REPORT, 1, 0xff, 10, 0x2b, 2], 1, 10, 0x2b).is_err());
    }

    #[test]
    fn superlight_dpi_range_uses_sensor_step() {
        let mut data = [0; 16];
        data[..7].copy_from_slice(&[0, 0, 100, 0xe0, 50, 0x64, 0]);
        let range = DpiRange::parse(data).unwrap();
        assert!(range.contains(100));
        assert!(range.contains(850));
        assert!(range.contains(25_600));
        assert!(!range.contains(801));
        assert!(!range.contains(0));
        data[4] = 0;
        assert!(DpiRange::parse(data).is_err());
    }
}
