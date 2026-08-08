//! Interactive frontend (feature = "window"): minifb window feeding the
//! same Deck ops the tape uses. Built on machines with a display; the
//! container CI never compiles this module.
#![cfg(feature = "window")]

use crate::{render, Deck};
use echelon_sim::Cmd;
use minifb::{Key, MouseButton, MouseMode, Window, WindowOptions};

pub fn run(mut deck: Deck) {
    let f0 = render::render(&deck.world, &deck.ts, deck.me, &[], &[], None, &[]);
    let (w, h) = (f0.w, f0.h);
    let mut win = Window::new("Echelon Command Deck", w, h, WindowOptions::default())
        .expect("window");
    win.limit_update_rate(Some(std::time::Duration::from_micros(33_000)));
    let mut drag: Option<(i32, i32)> = None;
    let mut buf32 = vec![0u32; w * h];
    while win.is_open() && !win.is_key_down(Key::Escape) {
        deck.step(2);
        let mouse = win.get_mouse_pos(MouseMode::Clamp).unwrap_or((0.0, 0.0));
        let (tx, ty) = (
            (mouse.0 as i32) / render::PX,
            (mouse.1 as i32) / render::PX,
        );
        if win.get_mouse_down(MouseButton::Left) {
            if drag.is_none() {
                drag = Some((tx, ty));
            }
        } else if let Some((sx, sy)) = drag.take() {
            deck.select_rect(sx, sy, tx, ty);
        }
        if win.get_mouse_down(MouseButton::Right) {
            let ids = deck.selection.clone();
            deck.cmd_pub(Cmd::Move { ids, mx: tx * 1000 + 500, my: ty * 1000 + 500 });
        }
        if win.is_key_pressed(Key::A, minifb::KeyRepeat::No) {
            let ids = deck.selection.clone();
            deck.cmd_pub(Cmd::Attack { ids, mx: tx * 1000 + 500, my: ty * 1000 + 500 });
        }
        if win.is_key_pressed(Key::S, minifb::KeyRepeat::No) {
            let ids = deck.selection.clone();
            deck.cmd_pub(Cmd::Stop { ids });
        }
        if win.is_key_pressed(Key::H, minifb::KeyRepeat::No) {
            let ids = deck.selection.clone();
            deck.cmd_pub(Cmd::Hold { ids });
        }
        if win.is_key_pressed(Key::R, minifb::KeyRepeat::No) {
            for s in deck.selection.clone() {
                deck.cmd_pub(Cmd::Rally { structure: s, mx: tx * 1000 + 500, my: ty * 1000 + 500 });
            }
        }
        let hud = deck.hud_lines_pub();
        let f = render::render(
            &deck.world,
            &deck.ts,
            deck.me,
            &deck.selection,
            &deck.ghosts,
            deck.banner.as_deref(),
            &hud,
        );
        for (i, px) in buf32.iter_mut().enumerate() {
            let o = i * 3;
            *px = ((f.buf[o] as u32) << 16) | ((f.buf[o + 1] as u32) << 8) | f.buf[o + 2] as u32;
        }
        win.update_with_buffer(&buf32, w, h).expect("blit");
    }
}
