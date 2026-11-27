//! Connects every `App` callback to the controller.

use std::cell::RefCell;
use std::rc::Rc;

use jiff::ZonedDateTime;
use patina_core::command::NextAction;
use slint::{ComponentHandle as _, Model as _, SharedString};

use crate::controller::Controller;
use crate::{
    App, DatePickerAdapter, HabitForm, ItemForm, LanguageUi, MainWindow, NoticeData, PickerPurpose,
    RhythmModeUi, convert,
};

/// Registers all callbacks of the `App` global on `window`.
pub fn connect(window: &MainWindow, ctl: Rc<RefCell<Controller>>) {
    let app = window.global::<App<'_>>();

    // `bind!(on_name, |c, arg: Type, ...| expr)`: calls the controller `c`.
    macro_rules! bind {
        ($on:ident, |$c:ident $(, $a:ident : $t:ty)*| $body:expr) => {{
            let ctl = ctl.clone();
            app.$on(move |$($a: $t),*| {
                let mut $c = ctl.borrow_mut();
                $body
            });
        }};
    }

    bind!(on_capture, |c, text: SharedString| c.capture(&text));
    bind!(on_switch_mode, |c, name: SharedString| c.switch_mode(&name));
    bind!(on_dismiss_suggestion, |c, name: SharedString| c
        .dismiss_suggestion(&name));
    bind!(on_set_scope, |c, kind: i32, name: SharedString| c
        .set_scope(kind, &name));
    bind!(on_navigate, |c, page: i32| c.navigate(page));
    bind!(on_refresh, |c| c.refresh());
    bind!(on_open_item, |c, id: SharedString| c.open_item(&id));
    bind!(on_close_detail, |c| c.close_detail());
    bind!(on_toggle_unit, |c, id: SharedString, index: i32| c
        .toggle_unit(&id, index));
    bind!(on_accept_proposal, |c, id: SharedString| c
        .accept_proposal(&id));
    bind!(on_reject_proposal, |c, id: SharedString| c
        .reject_proposal(&id));
    bind!(on_undo_source, |c, source: SharedString| c
        .undo_source(&source));
    bind!(on_open_graph, |c, id: SharedString| c.open_graph(&id));
    bind!(on_close_graph, |c| c.close_graph());
    bind!(on_open_settings, |c| c.set_settings_open(true));
    bind!(on_close_settings, |c| c.set_settings_open(false));
    bind!(on_undo, |c| {
        let _ = c.undo();
    });
    bind!(on_notice_action, |c, n: NoticeData| c.notice_action(&n));

    bind!(on_card_start, |c| {
        let _ = c.card_action(NextAction::Start);
    });
    bind!(on_card_skip, |c| {
        let _ = c.card_action(NextAction::Skip);
    });
    bind!(on_card_split, |c, text: SharedString| c.card_action(
        NextAction::Split {
            text: text.to_string()
        }
    ));

    bind!(on_start_timer, |c, id: SharedString| c
        .start_timer(&id, false));
    bind!(on_start_habit_timer, |c, id: SharedString| c
        .start_timer(&id, true));
    bind!(on_stop_timer, |c| c.stop_timer());
    bind!(on_stuck, |c,
                     item: SharedString,
                     waiting: SharedString,
                     follow_up: SharedString| c
        .stuck(&item, &waiting, &follow_up));
    bind!(
        on_interrupt,
        |c, text: SharedString, now: bool, identity: SharedString, left_off: SharedString| c
            .interrupt(&text, now, &identity, &left_off)
    );
    bind!(on_start_break, |c| c.start_break());
    bind!(on_snooze_break, |c| c.snooze_break());
    bind!(on_set_rhythm, |c, mode: RhythmModeUi| c
        .set_rhythm(convert::rhythm_mode_core(mode)));
    bind!(on_set_language, |c, lang: LanguageUi| c
        .set_language(convert::language_pref(lang)));
    bind!(on_resume, |c| c.resume());
    bind!(on_correct_timer, |c, t: SharedString| c.correct_timer(&t));
    bind!(on_keep_timer, |_c| {});
    bind!(on_correct_mode, |c, t: SharedString| c.correct_mode(&t));
    bind!(on_keep_mode, |c| c.keep_mode());

    bind!(on_mark_started, |c, id: SharedString| c.mark_started(&id));
    bind!(on_complete, |c, id: SharedString, note: SharedString| c
        .complete(&id, &note));
    bind!(on_abandon, |c,
                       id: SharedString,
                       reason: i32,
                       note: SharedString| c
        .abandon(&id, reason, &note));
    bind!(on_add_item, |c, form: ItemForm| c.add_item(&form));
    bind!(on_edit_item, |c, id: SharedString, form: ItemForm| c
        .edit_item(&id, &form));
    bind!(on_add_note, |c, id: SharedString, text: SharedString| c
        .add_note(&id, &text));
    bind!(on_move_item, |c, id: SharedString, parent: SharedString| c
        .move_item(&id, &parent));
    bind!(on_link, |c, id: SharedString, dep: SharedString| c
        .link(&id, &dep));
    bind!(on_unlink, |c, id: SharedString, dep: SharedString| c
        .unlink(&id, &dep));
    bind!(on_unlink_abandoned, |c, id: SharedString| c
        .unlink_abandoned(&id));
    bind!(
        on_picker_query,
        |c, purpose: PickerPurpose, filter: SharedString, context: SharedString| c
            .picker_query(purpose, &filter, &context)
    );

    bind!(on_triage, |c, id: SharedString, form: ItemForm| c
        .triage(&id, &form));
    bind!(on_discard, |c, id: SharedString| c.discard(&id));
    bind!(on_merge, |c, id: SharedString, into: SharedString| c
        .merge(&id, &into));

    bind!(on_check_in, |c,
                        id: SharedString,
                        offset: i32,
                        count: i32,
                        note: SharedString| c
        .check_in(&id, offset, count, &note));
    bind!(on_skip_habit, |c, id: SharedString, offset: i32| c
        .skip_habit(&id, offset));
    bind!(on_pause_habit, |c, id: SharedString| c
        .pause_habit(&id, true));
    bind!(on_resume_habit, |c, id: SharedString| c
        .pause_habit(&id, false));
    bind!(on_graduate_habit, |c, id: SharedString| c
        .graduate_habit(&id, true));
    bind!(on_reinstate_habit, |c, id: SharedString| c
        .graduate_habit(&id, false));
    bind!(on_add_habit, |c, form: HabitForm| c.add_habit(&form));
    bind!(on_edit_habit, |c,
                          id: SharedString,
                          name: SharedString,
                          identity: SharedString,
                          cue: SharedString| {
        c.edit_habit(&id, &name, &identity, &cue)
    });

    bind!(on_set_show_settled, |c, all: bool| c.set_show_settled(all));
    bind!(
        on_set_scheduled,
        |c, id: SharedString, date: SharedString| c.set_scheduled(&id, &date)
    );
    bind!(
        on_set_follow_up,
        |c, id: SharedString, date: SharedString| c.set_follow_up(&id, &date)
    );
    bind!(
        on_review_finish,
        |c, conclusion: SharedString, focus: slint::ModelRc<SharedString>, minimal: bool| c
            .review_finish(
                &conclusion,
                focus.iter().map(|s| s.to_string()).collect(),
                minimal
            )
    );
    bind!(on_init_identities, |c| c.init_identities());
    bind!(on_add_identity, |c, name: SharedString| c
        .add_identity(&name));
    bind!(
        on_rename_identity,
        |c, old: SharedString, new: SharedString| c.rename_identity(&old, &new)
    );
    bind!(
        on_set_target,
        |c, name: SharedString, hours: SharedString| c.set_target(&name, &hours)
    );

    // Data for Material's date picker; "now" is the system date.
    // The formatter reads a shared `Cell`, so this pure callback never borrows the controller.
    let locale = ctl.borrow().locale();
    let dp = window.global::<DatePickerAdapter<'_>>();
    dp.on_month_day_count(convert::picker_days_in_month);
    dp.on_month_offset(convert::picker_month_offset);
    dp.on_format_date(move |f, d, m, y| {
        convert::ss(convert::picker_format(locale.get(), &f, d, m, y))
    });
    dp.on_parse_date(|s, f| convert::model(convert::picker_parse(&s, &f)));
    dp.on_valid_date(|s, f| !convert::picker_parse(&s, &f).is_empty());
    dp.on_date_now(|| convert::model(convert::picker_parts(ZonedDateTime::now().date())));
}
