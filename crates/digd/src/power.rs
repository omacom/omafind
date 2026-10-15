//! Power state from D-Bus, pushed: UPower's `OnBattery` and power-profiles-daemon's `ActiveProfile`. Without
//! either service the machine counts as plugged in and balanced.

use std::sync::mpsc::Sender;

use crate::daemon::Msg;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PowerState {
    pub on_battery: bool,
    pub power_saver: bool,
}

/// Reads the current state, sends it, then forwards every change. Returns when D-Bus is unreachable.
pub fn watch(tx: Sender<Msg>) {
    let Ok(connection) = zbus::blocking::Connection::system() else {
        crate::log!("no system D-Bus: assuming AC power");
        return;
    };
    let upower = zbus::blocking::Proxy::new(&connection, "org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower").ok();
    let profiles = zbus::blocking::Proxy::new(&connection, "org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles")
        .ok()
        .filter(|p| p.get_property::<String>("ActiveProfile").is_ok())
        .or_else(|| zbus::blocking::Proxy::new(&connection, "net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles").ok());

    let read = |upower: &Option<zbus::blocking::Proxy>, profiles: &Option<zbus::blocking::Proxy>| PowerState {
        on_battery: upower.as_ref().and_then(|p| p.get_property::<bool>("OnBattery").ok()).unwrap_or(false),
        power_saver: profiles.as_ref().and_then(|p| p.get_property::<String>("ActiveProfile").ok()).is_some_and(|s| s == "power-saver"),
    };
    let state = read(&upower, &profiles);
    let _ = tx.send(Msg::Power(state));

    // One thread per property stream; each sends the full state when its property changes.
    let mut handles = Vec::new();
    if let Some(upower) = upower.clone() {
        let tx = tx.clone();
        let profiles = profiles.clone();
        handles.push(std::thread::spawn(move || {
            let mut stream = upower.receive_property_changed::<bool>("OnBattery");
            while let Some(changed) = stream.next() {
                if let Ok(on_battery) = changed.get() {
                    let power_saver = profiles.as_ref().and_then(|p| p.get_property::<String>("ActiveProfile").ok()).is_some_and(|s| s == "power-saver");
                    if tx.send(Msg::Power(PowerState { on_battery, power_saver })).is_err() {
                        return;
                    }
                }
            }
        }));
    }
    if let Some(profiles) = profiles.clone() {
        let tx = tx.clone();
        let upower = upower.clone();
        handles.push(std::thread::spawn(move || {
            let mut stream = profiles.receive_property_changed::<String>("ActiveProfile");
            while let Some(changed) = stream.next() {
                if let Ok(profile) = changed.get() {
                    let on_battery = upower.as_ref().and_then(|p| p.get_property::<bool>("OnBattery").ok()).unwrap_or(false);
                    if tx.send(Msg::Power(PowerState { on_battery, power_saver: profile == "power-saver" })).is_err() {
                        return;
                    }
                }
            }
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
}
