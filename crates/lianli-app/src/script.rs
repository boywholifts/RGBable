//! Script effects: a few lines of Rhai that give each LED its colour.
//!
//! The script runs once per LED per frame with these variables:
//! `t` (0 to 1 through the loop), `pos` (0 to 1 around the fan ring or along the cable),
//! `led` (index on the device), `zone` (fan or cable number), `count` (LEDs in the zone)
//! and `colors` (the effect's colours as `[r, g, b]` arrays). It returns `rgb(r, g, b)`,
//! `hsv(h, s, v)`, an `[r, g, b]` array or a `0xRRGGBB` number.

use parking_lot::Mutex;
use rhai::{Array, Dynamic, Engine, Scope, AST, FLOAT, INT};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

pub const MAX_SCRIPT_LEN: usize = 8 * 1024;
const MAX_OPERATIONS: u64 = 20_000;

pub const TEMPLATE: &str = "\
// Runs for every LED in every frame. Return rgb(r, g, b) with 0-255 values
// or hsv(hue, saturation, value) with 0-1 values.
//   t       how far through the loop, 0 to 1
//   pos     where the LED is around the fan or along the cable, 0 to 1
//   led     LED number, zone = fan or cable number, count = LEDs in the zone
//   colors  your colours: colors[0], colors[1], ...
let wave = sin((t + pos) * TAU) * 0.5 + 0.5;
mix(colors[0], colors[1], wave)
";

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_expr_depths(64, 32);
        engine.set_max_string_size(1024);
        engine.set_max_array_size(1024);
        engine.on_print(|_| {});
        engine.on_debug(|_, _, _| {});
        engine.register_fn("rgb", |r: INT, g: INT, b: INT| {
            color_array(r as FLOAT, g as FLOAT, b as FLOAT)
        });
        engine.register_fn("rgb", |r: FLOAT, g: FLOAT, b: FLOAT| color_array(r, g, b));
        engine.register_fn("hsv", |h: FLOAT, s: FLOAT, v: FLOAT| {
            let [r, g, b] = hsv(h, s, v);
            color_array(r, g, b)
        });
        engine.register_fn("mix", |a: Array, b: Array, t: FLOAT| {
            let (a, b) = (to_rgb(&a), to_rgb(&b));
            let t = t.clamp(0.0, 1.0);
            color_array(
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            )
        });
        engine.register_fn("dim", |c: Array, level: FLOAT| {
            let c = to_rgb(&c);
            let level = level.clamp(0.0, 1.0);
            color_array(c[0] * level, c[1] * level, c[2] * level)
        });
        engine.register_fn("palette", |colors: Array, x: FLOAT| {
            if colors.is_empty() {
                return color_array(255.0, 255.0, 255.0);
            }
            let x = (x - x.floor()) * colors.len() as FLOAT;
            let i = x.floor() as usize % colors.len();
            let next = (i + 1) % colors.len();
            let a = colors[i]
                .read_lock::<Array>()
                .map(|a| to_rgb(&a))
                .unwrap_or([255.0; 3]);
            let b = colors[next]
                .read_lock::<Array>()
                .map(|a| to_rgb(&a))
                .unwrap_or([255.0; 3]);
            let f = x.fract();
            let f = f * f * (3.0 - 2.0 * f);
            color_array(
                a[0] + (b[0] - a[0]) * f,
                a[1] + (b[1] - a[1]) * f,
                a[2] + (b[2] - a[2]) * f,
            )
        });
        engine.register_fn("fract", |x: FLOAT| x - x.floor());
        engine.register_fn("clamp01", |x: FLOAT| x.clamp(0.0, 1.0));
        engine.register_fn("smooth", |x: FLOAT| {
            let x = x.clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        });
        engine.register_fn("noise", |x: INT| {
            let mut v = (x as u32).wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
            v ^= v >> 15;
            v = v.wrapping_mul(0x2c1b_3c6d);
            ((v ^ (v >> 12)) & 0xffff) as FLOAT / 65536.0
        });
        engine
    })
}

fn color_array(r: FLOAT, g: FLOAT, b: FLOAT) -> Array {
    vec![
        Dynamic::from_float(r),
        Dynamic::from_float(g),
        Dynamic::from_float(b),
    ]
}

fn number(value: &Dynamic) -> FLOAT {
    value
        .as_float()
        .or_else(|_| value.as_int().map(|i| i as FLOAT))
        .unwrap_or(0.0)
}

fn to_rgb(array: &Array) -> [FLOAT; 3] {
    [0, 1, 2].map(|i| array.get(i).map(number).unwrap_or(0.0))
}

fn hsv(h: FLOAT, s: FLOAT, v: FLOAT) -> [FLOAT; 3] {
    let h = (h - h.floor()) * 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0) * 255.0);
    let f = h.fract();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn to_color(value: &Dynamic) -> [u8; 3] {
    if let Some(array) = value.read_lock::<Array>() {
        return to_rgb(&array).map(|c| c.round().clamp(0.0, 255.0) as u8);
    }
    if let Ok(hex) = value.as_int() {
        return [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8];
    }
    [0, 0, 0]
}

type Compiled = Result<Arc<AST>, String>;

fn compiled(source: &str) -> Compiled {
    static CACHE: OnceLock<Mutex<HashMap<String, Compiled>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().get(source) {
        return hit.clone();
    }
    let result = if source.len() > MAX_SCRIPT_LEN {
        Err(format!(
            "scripts can be at most {MAX_SCRIPT_LEN} characters"
        ))
    } else {
        engine()
            .compile(source)
            .map(Arc::new)
            .map_err(|e| e.to_string())
    };
    let mut cache = cache.lock();
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(source.to_string(), result.clone());
    result
}

/// Checks a script by compiling it and running it for a few LEDs.
pub fn check(source: &str, colors: &[[u8; 3]]) -> Result<(), String> {
    render_frame(source, colors, &[13, 13], 0.3).map(|_| ())
}

/// One frame of a script effect. `zones` are LED counts per fan or cable.
pub fn render_frame(
    source: &str,
    colors: &[[u8; 3]],
    zones: &[usize],
    t: f32,
) -> Result<Vec<[u8; 3]>, String> {
    let ast = compiled(source)?;
    // Only the user's own colours, so gradients and random picks never bring in new ones;
    // a single colour is repeated so `colors[1]` still works.
    let chosen: &[[u8; 3]] = if colors.is_empty() {
        &[[255, 255, 255]]
    } else {
        colors
    };
    let palette: Array = (0..chosen.len().max(2))
        .map(|i| chosen[i % chosen.len()])
        .map(|c| Dynamic::from_array(c.iter().map(|&v| Dynamic::from_int(INT::from(v))).collect()))
        .collect();
    let mut scope = Scope::new();
    scope.push_constant("PI", std::f32::consts::PI);
    scope.push_constant("TAU", std::f32::consts::TAU);
    scope.push("t", t);
    scope.push("pos", 0.0 as FLOAT);
    scope.push("led", 0 as INT);
    scope.push("zone", 0 as INT);
    scope.push("count", 0 as INT);
    scope.push("colors", palette);
    let base = scope.len();
    let mut frame = Vec::with_capacity(zones.iter().sum());
    let mut led = 0;
    for (zone, &count) in zones.iter().enumerate() {
        for i in 0..count {
            scope.rewind(base);
            scope.set_value("pos", i as FLOAT / count.max(1) as FLOAT);
            scope.set_value("led", led as INT);
            scope.set_value("zone", zone as INT);
            scope.set_value("count", count as INT);
            let value = engine()
                .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
                .map_err(|e| e.to_string())?;
            frame.push(to_color(&value));
            led += 1;
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_renders_a_gradient_between_the_colours() {
        let frame = render_frame(TEMPLATE, &[[255, 0, 0], [0, 0, 255]], &[4], 0.0).unwrap();
        assert_eq!(frame.len(), 4);
        assert_ne!(frame[0], frame[1]);
    }

    #[test]
    fn scripts_can_return_hex_and_hsv() {
        assert_eq!(
            render_frame("0xff8000", &[], &[1], 0.0).unwrap(),
            vec![[255, 128, 0]]
        );
        assert_eq!(
            render_frame("hsv(0.0, 1.0, 1.0)", &[], &[1], 0.0).unwrap(),
            vec![[255, 0, 0]]
        );
    }

    #[test]
    fn runaway_and_broken_scripts_fail_instead_of_hanging() {
        assert!(render_frame("loop { }", &[], &[1], 0.0).is_err());
        assert!(render_frame("rgb(1, 2", &[], &[1], 0.0).is_err());
    }
}
