use super::*;
use crate::i18n::Language;
use eframe::App as _;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

#[derive(Default)]
struct MemStorage(HashMap<String, String>);
impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {}
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
}

fn onboarding_app(size: [f32; 2], language: Language, updated: bool) -> Harness<'static, App> {
    onboarding_app_scaled(size, language, updated, 100)
}

fn onboarding_app_scaled(
    size: [f32; 2],
    language: Language,
    updated: bool,
    scale: u16,
) -> Harness<'static, App> {
    let (backend, _) = Backend::offline(crate::ui_tests::data());
    let h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(size)
        .with_theme(if updated {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        })
        .build_eframe(|cc| {
            cc.egui_ctx.set_theme(if updated {
                egui::ThemePreference::Dark
            } else {
                egui::ThemePreference::Light
            });
            crate::theme::install_fonts(&cc.egui_ctx);
            crate::theme::apply(&cc.egui_ctx);
            let mut state = UiState {
                language,
                check_updates: false,
                autostart: Some(false),
                prefer_administrator: Some(false),
                ..Default::default()
            };
            state.display.scale = scale;
            if updated {
                state.onboarding.seen_version = Some("0.28.0".into());
            }
            state.onboarding.schedule(env!("CARGO_PKG_VERSION"));
            App {
                backend,
                state,
                tray: None,
                frames: Default::default(),
            }
        });
    crate::inventory::set_test_inventory(&h.ctx, Some(Ok(b"[]")));
    h
}

#[test]
fn onboarding_runs_once_per_version_and_defers_hidden_launches() {
    let ctx = egui::Context::default();
    let mut state = UiState::default();
    state.onboarding.schedule("0.29.0");
    assert!(!state.onboarding.needs_draft(false));
    assert!(state.onboarding.needs_draft(true));
    let draft = state.setup_snapshot(&ctx);
    state.onboarding.begin(draft);
    state.onboarding.acknowledge();
    let mut storage = MemStorage::default();
    eframe::set_value(&mut storage, PREFS_KEY, &state.prefs());
    let mut restored = UiState::restored(Some(&storage));
    restored.onboarding.schedule("0.29.0");
    assert!(!restored.onboarding.needs_draft(true));
    restored.onboarding.schedule("0.30.0-beta.1");
    assert!(restored.onboarding.needs_draft(true));
    let draft = restored.setup_snapshot(&ctx);
    restored.onboarding.begin(draft);
    restored.onboarding.acknowledge();
    assert_eq!(
        restored.onboarding.seen_version.as_deref(),
        Some("0.30.0-beta.1")
    );
    restored.onboarding.request_manual("0.30.0-beta.1");
    assert!(restored.onboarding.needs_draft(true));
    use eframe::Storage as _;
    storage.set_string(
        PREFS_KEY,
        "(tray_on_close: false, process_descriptions: false)".into(),
    );
    let mut legacy = UiState::restored(Some(&storage));
    legacy.onboarding.schedule("0.29.0");
    let legacy_draft = legacy.setup_snapshot(&ctx);
    assert!(!legacy_draft.tray_on_close && !legacy_draft.descriptions);
    assert!(legacy.onboarding.needs_draft(true));
}

#[test]
fn onboarding_language_failure_preserves_choices_and_allows_retry() {
    let ctx = egui::Context::default();
    let mut state = UiState::default();
    state.onboarding.schedule("0.29.0");
    let before = state.prefs();
    let mut draft = state.setup_snapshot(&ctx);
    state.onboarding.begin(draft.clone());
    draft.language = Language::German;
    draft.theme = egui::ThemePreference::Dark;
    draft.display.scale = 125;
    draft.display.density = crate::preferences::Density::Comfortable;
    draft.mode = crate::processes::ViewMode::Tree;
    draft.inspector = crate::workspace_inspector::Mode::Hidden;
    draft.lines.rows = false;
    draft.numeric_tray.cpu = true;
    draft.numeric_tray.memory = true;
    draft.tray_on_close = false;
    draft.check_updates = false;
    draft.experience.active_seconds = 3;
    let error = state
        .apply_setup(&ctx, draft.clone(), |_| {
            Err(std::io::Error::other("fixture write denied"))
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "fixture write denied");
    assert_eq!(state.prefs(), before);
    assert_eq!(
        crate::theme::TableLines::load_context(&ctx),
        Default::default()
    );
    assert_eq!(
        ctx.options(|o| o.theme_preference),
        egui::ThemePreference::System
    );
    assert!(!state.onboarding.save_pending);
    state
        .apply_setup(&ctx, draft.clone(), |language| {
            assert_eq!(language, draft.language);
            Ok(())
        })
        .unwrap();
    assert_eq!(state.setup_snapshot(&ctx), draft);
    assert_eq!(state.onboarding.seen_version.as_deref(), Some("0.29.0"));
    assert!(state.onboarding.save_pending);
    let mut storage = MemStorage::default();
    eframe::set_value(&mut storage, PREFS_KEY, &state.prefs());
    assert_eq!(UiState::restored(Some(&storage)).prefs(), state.prefs());
}

#[test]
fn onboarding_next_back_skip_discards_draft_and_saves_marker() {
    let mut h = onboarding_app([1000.0, 740.0], Language::English, false);
    h.run_steps(3);
    assert!(h.query_by_label("Welcome to RustWTM").is_some());
    let before = h.state().state.setup_snapshot(&h.ctx);
    h.get_by_label("Next").click();
    h.run_steps(2);
    h.get_by_label("Dark").click();
    h.run_steps(2);
    h.get_by_label("Back").click();
    h.run_steps(2);
    assert!(h.query_by_label("Step 1 of 5").is_some());
    h.get_by_label("Next").click();
    h.run_steps(2);
    assert_eq!(
        h.get_by_label("Dark").accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    h.get_by_label("Skip setup").click();
    h.run_steps(3);
    assert!(h.query_by_label("Skip setup").is_none());
    assert_eq!(h.state().state.setup_snapshot(&h.ctx), before);
    assert_eq!(h.state().auto_save_interval(), Duration::ZERO);
    let mut storage = MemStorage::default();
    h.state_mut().save(&mut storage);
    assert_eq!(h.state().auto_save_interval(), Duration::from_secs(30));
    let mut restored = UiState::restored(Some(&storage));
    restored.onboarding.schedule(env!("CARGO_PKG_VERSION"));
    assert!(!restored.onboarding.needs_draft(true));
    h.state_mut().state.tab = Tab::Settings;
    h.run_steps(3);
    h.get_by_label("Quick setup").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Quick setup").click();
    h.run_steps(3);
    assert!(h.query_by_label("Skip setup").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Skip setup").is_none());
}

#[test]
fn onboarding_finish_applies_selected_settings_and_update_retains_them() {
    let mut h = onboarding_app([1000.0, 740.0], Language::English, true);
    h.run_steps(3);
    assert!(h.query_by_label("Welcome back").is_some());
    h.get_by_label("Next").click();
    h.run_steps(2);
    h.get_by_label("Comfortable").click();
    h.run_steps(2);
    h.get_by_label("Next").click();
    h.run_steps(2);
    h.get_by_label("Tree").click();
    h.run_steps(2);
    h.get_by_label("Row separators").click();
    h.run_steps(2);
    h.get_by_label("Next").click();
    h.run_steps(2);
    h.get_by_label("CPU usage number").click();
    h.run_steps(2);
    h.get_by_label("Next").click();
    h.run_steps(2);
    assert!(h.query_by_label("Review your settings").is_some());
    assert!(
        !h.state().state.numeric_tray.cpu,
        "draft remains uncommitted"
    );
    h.get_by_label("Apply and finish").click();
    h.run_steps(3);
    assert!(h.query_by_label("Apply and finish").is_none());
    assert!(h.state().state.numeric_tray.cpu);
    assert_eq!(
        h.state().state.processes.mode,
        crate::processes::ViewMode::Tree
    );
    assert_eq!(
        h.state().state.display.density,
        crate::preferences::Density::Comfortable
    );
    assert!(!crate::theme::TableLines::load_context(&h.ctx).rows);
    assert_eq!(
        h.state().state.onboarding.seen_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn onboarding_navigation_fits_compact_windows_and_turkish() {
    for (size, language) in [
        ([820.0, 500.0], Language::English),
        ([500.0, 500.0], Language::Turkish),
    ] {
        let mut h = onboarding_app(size, language, false);
        h.run_steps(3);
        let _language = crate::i18n::scope(language);
        for step in 0..5 {
            for key in [
                "Back",
                "Skip setup",
                if step < 4 { "Next" } else { "Apply and finish" },
            ] {
                let rect = h.get_by_label(crate::i18n::tr(key)).rect();
                assert!(
                    rect.left() >= 0.0
                        && rect.top() >= 0.0
                        && rect.right() <= size[0]
                        && rect.bottom() <= size[1],
                    "{key}: {rect:?} in {size:?}"
                );
            }
            if step < 4 {
                h.get_by_label(crate::i18n::tr("Next")).click();
                h.run_steps(2);
            }
        }
    }
}

#[test]
fn onboarding_navigation_fits_every_language_at_maximum_scale() {
    for language in Language::ALL
        .into_iter()
        .filter(|language| *language != Language::System)
    {
        let mut h = onboarding_app_scaled([820.0, 500.0], language, true, 150);
        h.run_steps(3);
        let _language = crate::i18n::scope(language);
        for step in 0..5 {
            let bounds = h.ctx.viewport_rect();
            let mut buttons = Vec::new();
            for key in [
                "Back",
                "Skip setup",
                if step < 4 { "Next" } else { "Apply and finish" },
            ] {
                let rect = h
                    .get_by_role_and_label(egui::accesskit::Role::Button, crate::i18n::tr(key))
                    .rect();
                assert!(
                    bounds.contains_rect(rect),
                    "{language:?}, step {step}, {key}: {rect:?} outside {bounds:?}"
                );
                assert!(
                    buttons
                        .iter()
                        .all(|previous: &egui::Rect| !previous.intersects(rect)),
                    "overlapping navigation: {language:?}, {step}"
                );
                buttons.push(rect);
            }
            if step < 4 {
                h.get_by_role_and_label(egui::accesskit::Role::Button, crate::i18n::tr("Next"))
                    .click();
                h.run_steps(2);
            }
        }
    }
}

#[test]
fn onboarding_review_edits_sections_without_committing_the_draft() {
    let mut h = onboarding_app([820.0, 500.0], Language::English, false);
    h.run_steps(3);
    let before = h.state().state.setup_snapshot(&h.ctx);
    for _ in 0..4 {
        h.get_by_label("Next").click();
        h.run_steps(2);
    }
    for (section, choice) in [
        ("Display and accessibility", Some("Dark")),
        ("Process workspace", Some("Tree")),
        ("Language", None),
        ("Window behavior", Some("CPU usage number")),
    ] {
        let label = format!("Edit: {section}");
        h.get_by_label(&label).scroll_to_me();
        // ScrollIntoView updates the offset after layout; render that offset
        // before reading the button's new pointer position.
        h.run_steps(3);
        h.get_by_label(&label).click();
        h.run_steps(2);
        assert!(
            h.query_by_role_and_label(egui::accesskit::Role::Button, "Review your settings")
                .is_some(),
            "edit must open {section}"
        );
        if let Some(choice) = choice {
            h.get_by_label(choice).click();
            h.run_steps(2);
        }
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Review your settings")
            .click();
        h.run_steps(2);
        assert!(h.query_by_label("Apply and finish").is_some());
        assert_eq!(
            h.state().state.setup_snapshot(&h.ctx),
            before,
            "review keeps changes in its draft"
        );
    }
    h.get_by_label("Apply and finish").click();
    h.run_steps(3);
    assert_eq!(
        h.ctx.options(|o| o.theme_preference),
        egui::ThemePreference::Dark
    );
    assert_eq!(
        h.state().state.processes.mode,
        crate::processes::ViewMode::Tree
    );
    assert!(h.state().state.numeric_tray.cpu);
}

#[test]
fn onboarding_accessibility_retry_error_is_visible_in_a_compact_review() {
    for (language, scale) in [(Language::English, 100), (Language::Turkish, 150)] {
        let mut h = onboarding_app_scaled([820.0, 500.0], language, false, scale);
        h.run_steps(3);
        let _language = crate::i18n::scope(language);
        for _ in 0..4 {
            h.get_by_label(crate::i18n::tr("Next")).click();
            h.run_steps(2);
        }
        let before = h.get_by_label(crate::i18n::tr("Apply and finish")).rect();
        let error = crate::i18n::format(
            "Cannot save language: {0}",
            &["fixture write denied".into()],
        );
        h.state_mut().state.onboarding.error = Some(error.clone());
        h.run_steps(3);
        let rect = h.get_by_label(&error).rect();
        let footer = h.get_by_label(crate::i18n::tr("Apply and finish")).rect();
        assert!(
            rect.top() >= 0.0 && rect.bottom() < footer.top(),
            "retry error must be visible above navigation: {rect:?}, footer {footer:?}"
        );
        assert!(
            (before.top() - footer.top()).abs() <= 1.0,
            "feedback keeps navigation fixed: {before:?}, {footer:?}"
        );
        assert!(h.ctx.viewport_rect().contains_rect(footer));
        assert!(h.state().state.onboarding.seen_version.is_none());
    }
}

#[test]
fn onboarding_accessibility_reduced_motion_stops_programmatic_scroll_animation() {
    let ctx = egui::Context::default();
    let mut display = crate::preferences::Display {
        reduced_motion: true,
        ..Default::default()
    };
    let mut applied = None;
    display.apply(&ctx, &mut applied);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let style = ctx.style_of(theme);
        assert_eq!(style.scroll_animation.duration.min, 0.0);
        assert_eq!(style.scroll_animation.duration.max, 0.0);
    }
    display.reduced_motion = false;
    display.apply(&ctx, &mut applied);
    assert!(
        ctx.style_of(egui::Theme::Light)
            .scroll_animation
            .duration
            .max
            > 0.0
    );
}

#[test]
fn onboarding_keyboard_navigation_keeps_popup_escape_inside_setup() {
    let mut h = onboarding_app([1000.0, 740.0], Language::English, false);
    h.run_steps(3);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().state.tab, Tab::Overview);
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Language")
        .click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert!(
        h.query_by_label("Skip setup").is_some(),
        "Escape first closes the language popup"
    );
    h.get_by_label("Next").focus();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.query_by_label("Step 2 of 5").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Skip setup").is_none());
    assert_eq!(
        h.state().state.onboarding.seen_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

#[cfg(feature = "visual-tests")]
#[test]
#[ignore = "bounded native GPU review; run explicitly with one test thread"]
fn render_onboarding_error_review() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/onboarding-error-review");
    std::fs::create_dir_all(&root).unwrap();
    for (language, scale, name) in [
        (Language::English, 100, "error-light"),
        (Language::Turkish, 150, "error-large-text-dark"),
    ] {
        let mut h = onboarding_app_scaled(
            [820.0, 500.0],
            language,
            language == Language::Turkish,
            scale,
        );
        let _language = crate::i18n::scope(language);
        h.run_steps(3);
        for _ in 0..4 {
            h.get_by_label(crate::i18n::tr("Next")).click();
            h.run_steps(2);
        }
        h.state_mut().state.onboarding.error = Some(crate::i18n::format(
            "Cannot save language: {0}",
            &["fixture write denied".into()],
        ));
        h.run_steps(3);
        let path = root.join(format!("{name}.png"));
        h.render().expect("GPU render").save(&path).unwrap();
        println!("Rendered {}", path.display());
    }
}

#[cfg(feature = "visual-tests")]
#[test]
#[ignore = "bounded native GPU review; run explicitly with one test thread"]
fn render_onboarding_review() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/onboarding-review");
    std::fs::create_dir_all(&root).unwrap();
    for (size, language, updated, name) in [
        ([1100.0, 780.0], Language::English, false, "welcome-light"),
        (
            [820.0, 500.0],
            Language::Turkish,
            true,
            "updated-compact-dark",
        ),
    ] {
        let mut h = onboarding_app(size, language, updated);
        h.run_steps(3);
        let _language = crate::i18n::scope(language);
        for step in 0..5 {
            let path = root.join(format!("{name}-{step}.png"));
            h.render().expect("GPU render").save(&path).unwrap();
            println!("Rendered {}", path.display());
            if step < 4 {
                h.get_by_label(crate::i18n::tr("Next")).click();
                h.run_steps(2);
            }
        }
    }
}
