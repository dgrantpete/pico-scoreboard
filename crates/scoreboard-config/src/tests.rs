//! What a corrupt config, a partial update and a hostile one each do.

use super::*;

extern crate std;
use std::string::String as StdString;

fn json(config: &DeviceConfig) -> StdString {
    let mut out = [0u8; 4096];
    let len = config.to_json(&mut out).unwrap();
    StdString::from_utf8(out[..len].to_vec()).unwrap()
}

fn patch(body: &str) -> ConfigPatch {
    ConfigPatch::from_json(body.as_bytes()).expect("patch should parse")
}

#[test]
fn defaults_match_the_micropython_defaults_dict() {
    let config = DeviceConfig::new();
    assert_eq!(config.network.device_name.as_str(), "scoreboard");
    assert_eq!(config.network.connect_timeout_seconds, 60);
    assert_eq!(config.display.brightness, 100);
    assert_eq!(config.display.poll_interval_seconds, 30);
    assert_eq!(config.display.game_rotation_seconds, 60);
    assert_eq!(config.display.data_frequency_khz, 20_000);
    assert_eq!(config.display.target_refresh_rate, 120.0);
    assert_eq!(config.display.gamma.kind, GammaKind::Srgb);
    assert_eq!(config.display.blanking_time_ns, 0);
    assert_eq!(config.display.variants.mlb_final.as_str(), "C");
    assert_eq!(config.display.variants.soccer_live.as_str(), "A");
    assert_eq!(config.display.variants.hockey_live.as_str(), "B");
    assert_eq!(config.display.variants.hockey_final.as_str(), "A");
    assert!(config.display.show_dividers);
    assert_eq!(
        config.display.scroll_speed_px_per_sec,
        scoreboard_render::geometry::DEFAULT_SCROLL_SPEED
    );
    assert_eq!(config.colors.accent, Rgb::new(255, 255, 0));
    assert!(config.sports.mlb.enabled);
    assert!(!config.sports.nba.enabled);
    assert!(config.sports.nhl.enabled, "NHL.md D5: on by default");
    assert_eq!(config.log.level.as_str(), "debug");
    assert_eq!(config.server.cache_max_age_seconds, 600);
    assert!(!config.watchdog.enabled);
    assert_eq!(config.watchdog.timeout_ms, 8_000);
    assert!(config.ota.enabled);
}

#[test]
fn the_serialized_shape_carries_every_key_the_spa_reads() {
    let text = json(&DeviceConfig::new());
    for key in [
        "\"network\"",
        "\"ssid\"",
        "\"password\"",
        "\"device_name\"",
        "\"connect_timeout_seconds\"",
        "\"api\"",
        "\"url\"",
        "\"key\"",
        "\"display\"",
        "\"brightness\"",
        "\"poll_interval_seconds\"",
        "\"game_rotation_seconds\"",
        "\"data_frequency_khz\"",
        "\"target_refresh_rate\"",
        "\"gamma\"",
        "\"type\"",
        "\"blanking_time_ns\"",
        "\"variants\"",
        "\"mlb_final\"",
        "\"soccer_live\"",
        "\"hockey_live\"",
        "\"hockey_final\"",
        "\"show_dividers\"",
        "\"scroll_speed_px_per_sec\"",
        "\"colors\"",
        "\"primary\"",
        "\"clock_warning\"",
        "\"sports\"",
        "\"mlb\"",
        "\"nhl\"",
        "\"football\"",
        "\"leagues\"",
        "\"log\"",
        "\"level\"",
        "\"server\"",
        "\"cache_max_age_seconds\"",
        "\"watchdog\"",
        "\"timeout_ms\"",
        "\"ota\"",
        "\"enabled\"",
    ] {
        assert!(text.contains(key), "missing {key} in {text}");
    }
    // A default gamma is `{"type":"srgb"}` with no `value`, exactly as
    // `_DEFAULTS` spells it.
    assert!(text.contains(r#""gamma":{"type":"srgb"}"#), "{text}");
}

#[test]
fn a_stored_config_round_trips() {
    let mut config = DeviceConfig::new();
    let _ = config.network.ssid.push_str("HOME-NETWORK-5G");
    let _ = config.api.url.push_str("http://backend.example/api");
    config.display.brightness = 60;
    config.display.gamma = GammaConfig {
        kind: GammaKind::Power,
        value: Some(2.4),
    };
    let _ = config.sports.football.leagues.push({
        let mut slug = heapless::String::new();
        let _ = slug.push_str("college-football");
        slug
    });

    let text = json(&config);
    let (parsed, complaint) = DeviceConfig::from_json(text.as_bytes());
    assert_eq!(complaint, None);
    assert_eq!(parsed, config);
}

#[test]
fn a_partial_stored_document_takes_defaults_for_everything_absent() {
    // This is the deep merge: one key present, and every other key in the
    // document — including siblings inside the same section — defaults.
    let (config, complaint) =
        DeviceConfig::from_json(br#"{"display":{"brightness":25},"network":{"ssid":"HOME"}}"#);
    assert_eq!(complaint, None);
    assert_eq!(config.display.brightness, 25);
    assert_eq!(config.display.poll_interval_seconds, 30);
    assert_eq!(config.display.variants.mlb_final.as_str(), "C");
    assert_eq!(config.network.ssid.as_str(), "HOME");
    assert_eq!(config.network.device_name.as_str(), "scoreboard");
    assert_eq!(config.server.cache_max_age_seconds, 600);
}

/// A document the firmware before hockey stored, byte for byte: its
/// `to_json` after a fresh device's first settings-page save — NBA on, the
/// NFL, MLS and the Premier League picked, NBA's final on A. This crate's
/// serializer last changed in `3259128`, ahead of the 2026.10.12-03942fd
/// release on the units, so it is what every unit in the field has in flash.
const STORED_BEFORE_HOCKEY: &str = concat!(
    r#"{"network":{"ssid":"HOME-NETWORK-5G","password":"correct-horse-battery","#,
    r#""device_name":"scoreboard","connect_timeout_seconds":60},"#,
    r#""api":{"url":"https://pico-scoreboard-backend.fly.dev","key":"k3y"},"#,
    r#""display":{"brightness":100,"poll_interval_seconds":30,"game_rotation_seconds":60,"#,
    r#""data_frequency_khz":20000,"target_refresh_rate":120.0,"gamma":{"type":"srgb"},"#,
    r#""blanking_time_ns":0,"#,
    r#""variants":{"mlb_final":"C","nba_final":"A","football_final":"C","soccer_live":"A"},"#,
    r#""show_dividers":true,"scroll_speed_px_per_sec":30},"#,
    r#""colors":{"primary":{"r":255,"g":255,"b":255},"secondary":{"r":128,"g":128,"b":128},"#,
    r#""accent":{"r":255,"g":255,"b":0},"clock_normal":{"r":0,"g":255,"b":0},"#,
    r#""clock_warning":{"r":255,"g":10,"b":10}},"#,
    r#""sports":{"mlb":{"enabled":true},"nba":{"enabled":true},"#,
    r#""football":{"leagues":["nfl"]},"soccer":{"leagues":["usa.1","eng.1"]}},"#,
    r#""log":{"level":"debug"},"server":{"cache_max_age_seconds":600},"#,
    r#""watchdog":{"enabled":false,"timeout_ms":8000},"#,
    r#""ota":{"enabled":true,"channel":"stable"}}"#,
);

/// The same release's document with every league its settings page offered
/// switched on: eight sources, the most it could configure.
const STORED_EVERY_LEAGUE_BEFORE_HOCKEY: &str = concat!(
    r#"{"network":{"ssid":"HOME-NETWORK-5G","password":"correct-horse-battery","#,
    r#""device_name":"scoreboard","connect_timeout_seconds":60},"#,
    r#""api":{"url":"https://pico-scoreboard-backend.fly.dev","key":"k3y"},"#,
    r#""display":{"brightness":100,"poll_interval_seconds":30,"game_rotation_seconds":60,"#,
    r#""data_frequency_khz":20000,"target_refresh_rate":120.0,"gamma":{"type":"srgb"},"#,
    r#""blanking_time_ns":0,"#,
    r#""variants":{"mlb_final":"C","nba_final":"C","football_final":"C","soccer_live":"A"},"#,
    r#""show_dividers":true,"scroll_speed_px_per_sec":30},"#,
    r#""colors":{"primary":{"r":255,"g":255,"b":255},"secondary":{"r":128,"g":128,"b":128},"#,
    r#""accent":{"r":255,"g":255,"b":0},"clock_normal":{"r":0,"g":255,"b":0},"#,
    r#""clock_warning":{"r":255,"g":10,"b":10}},"#,
    r#""sports":{"mlb":{"enabled":true},"nba":{"enabled":true},"#,
    r#""football":{"leagues":["nfl","college-football"]},"#,
    r#""soccer":{"leagues":["usa.1","eng.1","mex.1","fifa.world"]}},"#,
    r#""log":{"level":"debug"},"server":{"cache_max_age_seconds":600},"#,
    r#""watchdog":{"enabled":false,"timeout_ms":8000},"#,
    r#""ota":{"enabled":true,"channel":"stable"}}"#,
);

fn source_keys(config: &DeviceConfig) -> std::vec::Vec<StdString> {
    let built = config.league_sources();
    assert_eq!(built.dropped, 0, "every configured league should fit");
    built
        .sources
        .iter()
        .map(|league| StdString::from(league.key.as_str()))
        .collect()
}

#[test]
fn a_config_stored_before_hockey_loads_with_the_nhl_on() {
    let (config, complaint) = DeviceConfig::from_json(STORED_BEFORE_HOCKEY.as_bytes());
    assert_eq!(complaint, None);

    // `sports` is present and `nhl` is not: the per-key default, not the
    // section's, is what has to answer — and it answers D5's "on".
    assert!(config.sports.nhl.enabled);
    assert_eq!(config.display.variants.hockey_live.as_str(), "B");
    assert_eq!(config.display.variants.hockey_final.as_str(), "A");

    // Everything the document did say survives.
    assert_eq!(config.network.ssid.as_str(), "HOME-NETWORK-5G");
    assert!(config.sports.mlb.enabled && config.sports.nba.enabled);
    assert_eq!(config.display.variants.nba_final.as_str(), "A");

    // The key now goes out on `GET /api/config`, which is how the settings
    // page learns this firmware has hockey (D7).
    assert!(json(&config).contains(r#""nhl":{"enabled":true}"#));

    // And the rotation picks hockey up in D6 order.
    assert_eq!(
        source_keys(&config),
        [
            "baseball/mlb",
            "basketball/nba",
            "hockey/nhl",
            "football/nfl",
            "soccer/usa.1",
            "soccer/eng.1",
        ]
    );
}

#[test]
fn a_unit_with_every_league_on_keeps_all_of_them_when_the_nhl_joins() {
    // The upgrade a ninth source would have broken: the eight leagues this
    // unit polled yesterday plus the NHL D5 switches on. With an eight-slot
    // slate the World Cup fell off the end, silently.
    let (config, complaint) = DeviceConfig::from_json(STORED_EVERY_LEAGUE_BEFORE_HOCKEY.as_bytes());
    assert_eq!(complaint, None);
    assert_eq!(
        source_keys(&config),
        [
            "baseball/mlb",
            "basketball/nba",
            "hockey/nhl",
            "football/nfl",
            "football/college-football",
            "soccer/usa.1",
            "soccer/eng.1",
            "soccer/mex.1",
            "soccer/fifa.world",
        ]
    );
}

#[test]
fn every_registered_league_fits_the_slate() {
    // Switch on every league the firmware knows by name. A tenth registered
    // league fails here: bump `slate::MAX_SOURCES` and re-measure the poller
    // arena (BUDGET.md) in the same change.
    let mut config = DeviceConfig::new();
    for league in scoreboard_model::feed::LEAGUES {
        let slug = heapless::String::try_from(league.slug).expect("a slug fits MAX_LEAGUE");
        match league.sport {
            Sport::Mlb => config.sports.mlb.enabled = true,
            Sport::Nba => config.sports.nba.enabled = true,
            Sport::Hockey => {
                assert_eq!(
                    league.slug, "nhl",
                    "a second hockey league needs a config shape to switch it on (NHL.md D5)"
                );
                config.sports.nhl.enabled = true;
            }
            Sport::Football => config.sports.football.leagues.push(slug).unwrap(),
            Sport::Soccer => config.sports.soccer.leagues.push(slug).unwrap(),
        }
    }

    let built = config.league_sources();
    assert_eq!(
        built.dropped,
        0,
        "{} leagues are registered and the slate holds {}",
        scoreboard_model::feed::LEAGUES.len(),
        MAX_SOURCES
    );
    assert_eq!(
        MAX_SOURCES,
        scoreboard_model::feed::LEAGUES.len(),
        "MAX_SOURCES is sized to exactly the registered leagues, no headroom"
    );
    // The registry is written in D6 order, so the sources come out in it.
    let keys: std::vec::Vec<&str> = built.sources.iter().map(|l| l.key.as_str()).collect();
    let registered: std::vec::Vec<StdString> = scoreboard_model::feed::LEAGUES
        .iter()
        .map(|league| StdString::from(LeagueId::from_slug(league.sport, league.slug).key.as_str()))
        .collect();
    assert_eq!(keys, registered);
}

#[test]
fn the_default_rotation_is_mlb_then_the_nhl() {
    assert_eq!(
        source_keys(&DeviceConfig::new()),
        ["baseball/mlb", "hockey/nhl"]
    );
}

#[test]
fn the_nhl_source_is_the_endpoint_the_backend_serves() {
    // The poller builds `/{key}/games` and `/{key}/games/{id}` from this key
    // verbatim, so this is the request path: `/hockey/nhl/games`.
    let (config, _) = DeviceConfig::from_json(br#"{"sports":{"mlb":{"enabled":false}}}"#);
    let built = config.league_sources();
    assert_eq!(built.sources.len(), 1);
    let nhl = &built.sources[0];
    assert_eq!(nhl.key.as_str(), "hockey/nhl");
    assert_eq!(nhl.sport, Sport::Hockey);
    assert_eq!(nhl.display_name.as_str(), "NHL", "the menu row's label");
}

#[test]
fn a_hand_written_config_past_the_slate_counts_what_it_drops() {
    // Unregistered slugs poll by design, so a raw `PUT` can name more leagues
    // than the slate holds. The overflow is counted for the log, never a panic
    // (that would be a boot loop), and what fits is the D6-ordered prefix.
    let (config, complaint) = DeviceConfig::from_json(
        br#"{"sports":{"nba":{"enabled":true},
            "football":{"leagues":["nfl","college-football","ufl"]},
            "soccer":{"leagues":["usa.1","eng.1","esp.1","ger.1","ita.1"]}}}"#,
    );
    assert_eq!(complaint, None);
    let built = config.league_sources();
    assert_eq!(built.sources.len(), MAX_SOURCES);
    assert_eq!(built.dropped, 2, "11 configured, 9 fit");
    let keys: std::vec::Vec<&str> = built.sources.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "baseball/mlb",
            "basketball/nba",
            "hockey/nhl",
            "football/nfl",
            "football/college-football",
            "football/ufl",
            "soccer/usa.1",
            "soccer/eng.1",
            "soccer/esp.1",
        ]
    );
}

#[test]
fn switching_the_nhl_off_sticks_across_a_reboot() {
    let mut config = DeviceConfig::new();
    let applied = config
        .apply(&patch(r#"{"sports":{"nhl":{"enabled":false}}}"#))
        .unwrap();
    assert!(!config.sports.nhl.enabled);
    assert!(config.sports.mlb.enabled, "the patch named only the NHL");
    // Sources are read once at poller start, so there is nothing to apply
    // live; the settings page prompts for the reboot.
    assert!(!applied.any());

    // The stored `false` is a present key, so the default never overrides it.
    let (reloaded, complaint) = DeviceConfig::from_json(json(&config).as_bytes());
    assert_eq!(complaint, None);
    assert!(!reloaded.sports.nhl.enabled);
    assert_eq!(source_keys(&reloaded), ["baseball/mlb"]);
}

#[test]
fn a_corrupt_document_falls_back_to_defaults_rather_than_failing() {
    // `Config()` is built before anything else at boot; a raise here is a
    // device that will not start.
    for broken in [
        &b"not json at all"[..],
        b"{",
        b"",
        b"[1,2,3]",
        br#"{"display":"not a section"}"#,
    ] {
        let (config, complaint) = DeviceConfig::from_json(broken);
        assert_eq!(complaint, Some(LoadComplaint::Unparseable), "for {broken:?}");
        assert_eq!(config, DeviceConfig::new());
    }
}

#[test]
fn an_invalid_stored_cadence_resets_only_the_two_cadence_keys() {
    let (config, complaint) = DeviceConfig::from_json(
        br#"{"network":{"ssid":"KEEPME"},"display":{"poll_interval_seconds":90,"game_rotation_seconds":60,"brightness":42}}"#,
    );
    assert_eq!(
        complaint,
        Some(LoadComplaint::InvalidCadence(CadenceError {
            poll_interval_seconds: 90,
            game_rotation_seconds: 60,
        }))
    );
    assert_eq!(config.display.poll_interval_seconds, 30);
    assert_eq!(config.display.game_rotation_seconds, 60);
    // The rest of the document survived — one bad edit does not cost the SSID.
    assert_eq!(config.network.ssid.as_str(), "KEEPME");
    assert_eq!(config.display.brightness, 42);
}

#[test]
fn an_unknown_gamma_type_degrades_to_srgb() {
    let (config, complaint) =
        DeviceConfig::from_json(br#"{"display":{"gamma":{"type":"wildly-invalid"}}}"#);
    assert_eq!(complaint, None);
    assert_eq!(config.display.gamma.kind, GammaKind::Srgb);
}

#[test]
fn a_power_gamma_without_a_value_uses_2_2() {
    let (config, _) = DeviceConfig::from_json(br#"{"display":{"gamma":{"type":"power"}}}"#);
    assert_eq!(config.display.gamma.kind, GammaKind::Power);
    assert_eq!(config.display.gamma.power_exponent(), 2.2);
}

#[test]
fn applying_a_patch_sets_only_what_it_names() {
    let mut config = DeviceConfig::new();
    let applied = config
        .apply(&patch(r#"{"display":{"brightness":15}}"#))
        .unwrap();
    assert_eq!(config.display.brightness, 15);
    assert_eq!(config.display.poll_interval_seconds, 30);
    // Brightness is not a live-apply hook — the auto-brightness loop owns the
    // driver call and re-reads config every tick.
    assert!(!applied.any());
}

#[test]
fn the_cadence_is_validated_against_the_merged_pair_not_the_patch_alone() {
    let mut config = DeviceConfig::new();
    // 45 alone is fine against the stored rotation of 60.
    assert!(
        config
            .apply(&patch(r#"{"display":{"poll_interval_seconds":45}}"#))
            .is_ok()
    );
    assert_eq!(config.display.poll_interval_seconds, 45);

    // Lowering rotation under the *stored* poll interval is what fails, even
    // though the patch says nothing about the poll interval.
    let error = config
        .apply(&patch(r#"{"display":{"game_rotation_seconds":30}}"#))
        .unwrap_err();
    assert_eq!(
        error,
        CadenceError {
            poll_interval_seconds: 45,
            game_rotation_seconds: 30,
        }
    );

    // And a jointly-valid pair is accepted however the keys are ordered — the
    // bug `update_many`'s merged-pair check was written to avoid.
    assert!(
        config
            .apply(&patch(
                r#"{"display":{"game_rotation_seconds":30,"poll_interval_seconds":10}}"#
            ))
            .is_ok()
    );
    assert_eq!(config.display.game_rotation_seconds, 30);
    assert_eq!(config.display.poll_interval_seconds, 10);
}

#[test]
fn equal_poll_and_rotation_is_rejected_because_the_bound_is_strict() {
    let mut config = DeviceConfig::new();
    assert!(
        config
            .apply(&patch(
                r#"{"display":{"poll_interval_seconds":60,"game_rotation_seconds":60}}"#
            ))
            .is_err()
    );
}

#[test]
fn a_rejected_patch_changes_nothing_at_all() {
    let mut config = DeviceConfig::new();
    let before = config.clone();
    // Brightness and the SSID are both valid and both in the same request as
    // the cadence that is not.
    let result = config.apply(&patch(
        r#"{"network":{"ssid":"NEW"},"display":{"brightness":5,"poll_interval_seconds":90}}"#,
    ));
    assert!(result.is_err());
    assert_eq!(config, before, "a rejected PUT must be a no-op");
}

#[test]
fn live_apply_flags_track_which_keys_the_request_named() {
    let mut config = DeviceConfig::new();

    let applied = config.apply(&patch(r#"{"colors":{"accent":{"r":1,"g":2,"b":3}}}"#)).unwrap();
    assert!(applied.ui_colors);
    assert!(!applied.touches_core1());
    assert_eq!(config.colors.accent, Rgb::new(1, 2, 3));

    let applied = config
        .apply(&patch(r#"{"display":{"show_dividers":false}}"#))
        .unwrap();
    assert!(applied.render_settings);
    assert!(!applied.gamma);

    let applied = config
        .apply(&patch(r#"{"display":{"gamma":{"type":"none"}}}"#))
        .unwrap();
    assert!(applied.gamma);
    assert!(!applied.render_settings);

    let applied = config
        .apply(&patch(
            r#"{"display":{"data_frequency_khz":15000,"target_refresh_rate":90,"blanking_time_ns":150}}"#,
        ))
        .unwrap();
    assert!(applied.data_clock && applied.refresh_rate && applied.blanking_time);
    assert!(applied.touches_core1());

    let applied = config.apply(&patch(r#"{"log":{"level":"error"}}"#)).unwrap();
    assert!(applied.log_level);
    assert_eq!(config.log_level(), LogLevel::Error);
}

#[test]
fn a_variants_patch_reapplies_render_settings_even_when_it_names_one_screen() {
    let mut config = DeviceConfig::new();
    let applied = config
        .apply(&patch(r#"{"display":{"variants":{"soccer_live":"B"}}}"#))
        .unwrap();
    assert!(applied.render_settings);
    assert_eq!(config.display.variants.soccer_live.as_str(), "B");
    // The screens the patch did not name keep their stored letters.
    assert_eq!(config.display.variants.mlb_final.as_str(), "C");
}

#[test]
fn the_hockey_layouts_reach_the_render_settings() {
    use scoreboard_render::geometry::HockeyLiveVariant;

    let mut config = DeviceConfig::new();
    assert_eq!(config.render_settings().hockey_live, HockeyLiveVariant::B);

    let applied = config
        .apply(&patch(r#"{"display":{"variants":{"hockey_live":"A"}}}"#))
        .unwrap();
    assert!(applied.render_settings);
    assert_eq!(config.display.variants.hockey_live.as_str(), "A");
    assert_eq!(config.display.variants.hockey_final.as_str(), "A");
    assert_eq!(config.render_settings().hockey_live, HockeyLiveVariant::A);

    // A letter hockey has no design for leaves the screen as it was, as an
    // unknown final letter does.
    config
        .apply(&patch(r#"{"display":{"variants":{"hockey_live":"C"}}}"#))
        .unwrap();
    assert_eq!(config.render_settings().hockey_live, HockeyLiveVariant::B);
}

#[test]
fn an_empty_body_is_an_empty_patch() {
    let mut config = DeviceConfig::new();
    let before = config.clone();
    for body in ["", "   ", "{}"] {
        let applied = config.apply(&patch(body)).unwrap();
        assert!(!applied.any());
    }
    assert_eq!(config, before);
}

#[test]
fn unknown_sections_and_keys_are_ignored() {
    let mut config = DeviceConfig::new();
    let applied = config
        .apply(&patch(
            r#"{"nonsense":{"a":1},"display":{"brightness":7,"invented_key":true}}"#,
        ))
        .unwrap();
    assert_eq!(config.display.brightness, 7);
    assert!(!applied.any());
}

#[test]
fn a_wrongly_typed_value_is_rejected_rather_than_stored() {
    // The deliberate deviation from MicroPython, which stored anything and let
    // the accessors cope. PARITY.md records it.
    assert!(ConfigPatch::from_json(br#"{"display":{"brightness":"bright"}}"#).is_err());
}

#[test]
fn render_settings_come_out_of_the_config() {
    let mut config = DeviceConfig::new();
    config.display.show_dividers = false;
    config.display.scroll_speed_px_per_sec = 10;
    config.display.variants.soccer_live.clear();
    let _ = config.display.variants.soccer_live.push_str("B");

    let settings = config.render_settings();
    assert!(!settings.show_dividers);
    assert_eq!(settings.scroll_px_per_second, 10);
    assert_ne!(
        settings.soccer_live_table(),
        scoreboard_render::RenderSettings::new().soccer_live_table()
    );
}

#[test]
fn an_illegal_scroll_speed_degrades_rather_than_being_rejected() {
    let mut config = DeviceConfig::new();
    // Not a member of the smooth set; `set_scroll_speed` snaps it back. The
    // stored value is left alone — `watchdog_timeout_ms` clamps on read the
    // same way, and a config file is a record of what was asked for.
    config.display.scroll_speed_px_per_sec = 37;
    assert_eq!(
        config.render_settings().scroll_px_per_second,
        scoreboard_render::geometry::DEFAULT_SCROLL_SPEED
    );
    assert_eq!(config.display.scroll_speed_px_per_sec, 37, "nothing is rewritten");
}

#[test]
fn the_speed_the_parity_release_stored_survives_the_upgrade_as_the_default() {
    // The one real device carrying this config has 40 px/s stored, which 60 FPS
    // made illegal. It must not stutter and it must not be rejected: it takes
    // the ordinary degrade path to the nearest legal speed. The stored 40
    // remains in the file until somebody saves the form again.
    let mut config = DeviceConfig::new();
    config.display.scroll_speed_px_per_sec = 40;
    assert_eq!(
        config.render_settings().scroll_px_per_second,
        scoreboard_render::geometry::DEFAULT_SCROLL_SPEED
    );
}

#[test]
fn an_unknown_variant_letter_leaves_that_screen_alone() {
    let mut config = DeviceConfig::new();
    config.display.variants.mlb_final.clear();
    let _ = config.display.variants.mlb_final.push_str("Z");
    let settings = config.render_settings();
    assert_eq!(
        settings.final_table(scoreboard_model::Sport::Mlb),
        scoreboard_render::RenderSettings::new().final_table(scoreboard_model::Sport::Mlb)
    );
}

#[test]
fn the_watchdog_timeout_is_clamped_to_what_the_rp2350_can_arm() {
    let mut config = DeviceConfig::new();
    config.watchdog.timeout_ms = 100;
    assert_eq!(config.watchdog_timeout_ms(), WATCHDOG_TIMEOUT_MIN_MS);
    config.watchdog.timeout_ms = 60_000;
    assert_eq!(config.watchdog_timeout_ms(), WATCHDOG_TIMEOUT_MAX_MS);
    config.watchdog.timeout_ms = 4_000;
    assert_eq!(config.watchdog_timeout_ms(), 4_000);
}

#[test]
fn blank_league_slugs_are_filtered_out() {
    let (config, _) = DeviceConfig::from_json(
        br#"{"sports":{"soccer":{"leagues":["usa.1","","eng.1"]},"football":{"leagues":[]}}}"#,
    );
    let soccer: std::vec::Vec<&str> = config.sports.soccer.active().collect();
    assert_eq!(soccer, std::vec!["usa.1", "eng.1"]);
    assert_eq!(config.sports.football.active().count(), 0);
}

#[test]
fn ui_colors_reach_the_model() {
    let mut config = DeviceConfig::new();
    config.colors.primary = Rgb::new(10, 20, 30);
    let colors = config.ui_colors();
    assert_eq!(colors.primary, scoreboard_model::Rgb888::new(10, 20, 30));
}

#[test]
fn a_too_long_string_is_rejected_rather_than_silently_truncated() {
    // A 40-character SSID cannot be joined anyway, and storing a truncated one
    // would produce a device that reports a network it is not trying to join.
    let long = "x".repeat(MAX_SSID + 8);
    let body = std::format!(r#"{{"network":{{"ssid":"{long}"}}}}"#);
    assert!(ConfigPatch::from_json(body.as_bytes()).is_err());
}

#[test]
fn serializing_into_too_small_a_buffer_reports_rather_than_truncating() {
    let mut out = [0u8; 32];
    assert_eq!(DeviceConfig::new().to_json(&mut out), Err(SerializeError));
}

#[test]
fn a_maximally_full_config_fits_the_storage_and_response_buffers() {
    // Both `scoreboard-app`'s `storage::BUFFER_BYTES` (3 KB) and its HTTP
    // response scratch (3 KB) are sized against "about 1.3 KB with every league
    // slot full". That is a claim about this type, so it is checked here rather
    // than discovered on a device that quietly stops saving its configuration.
    let mut config = DeviceConfig::new();
    fill(&mut config.network.ssid);
    fill(&mut config.network.password);
    fill(&mut config.network.device_name);
    fill(&mut config.api.url);
    fill(&mut config.api.key);
    config.display.gamma = GammaConfig {
        kind: GammaKind::Power,
        value: Some(2.2222222),
    };
    for variant in [
        &mut config.display.variants.mlb_final,
        &mut config.display.variants.nba_final,
        &mut config.display.variants.football_final,
        &mut config.display.variants.soccer_live,
        &mut config.display.variants.hockey_live,
        &mut config.display.variants.hockey_final,
    ] {
        fill(variant);
    }
    for sport in [&mut config.sports.football, &mut config.sports.soccer] {
        while sport.leagues.len() < MAX_LEAGUES {
            let mut slug = heapless::String::new();
            fill(&mut slug);
            let _ = sport.leagues.push(slug);
        }
    }
    fill(&mut config.log.level);

    let mut out = [0u8; 4096];
    let len = config
        .to_json(&mut out)
        .expect("the largest possible document must serialize");
    assert!(
        len <= 3 * 1024,
        "a full configuration is {len} B, over the 3 KB both buffers allow"
    );
    // And it must come back, because that is what a boot does with it.
    let (parsed, complaint) = DeviceConfig::from_json(&out[..len]);
    assert_eq!(complaint, None);
    assert_eq!(parsed, config);
}

/// Pack a bounded string to its capacity with `'x'`.
fn fill<const N: usize>(text: &mut heapless::String<N>) {
    text.clear();
    while text.push('x').is_ok() {}
}
