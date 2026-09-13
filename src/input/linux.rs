//! Linux input capture: currently a no-op. Global input polling needs
//! desktop-host specific facilities (EVDEV / X hooks) we don't yet ship, so
//! capturing events is disabled and the overlay draws nothing. The capture
//! pipeline itself is unaffected.