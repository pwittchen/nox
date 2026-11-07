//! Hot-path benches (SPEC §12): VT parsing throughput on synthetic streams that mimic `cat`
//! of source code, colored `ls`, `htop` redraws and `vim` scrolling; the visible-cell copy the
//! renderer does each frame; and the key encoder. Run with `cargo bench -p nox_terminal`.

use std::hint::black_box;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nox_terminal::keys::{KeyInput, KeyModes, Mods, encode};
use nox_terminal::osc::OscSniffer;

struct Size(usize, usize);
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

fn source_code(bytes: usize) -> Vec<u8> {
    let line = b"    pub fn advance<H: Handler>(&mut self, handler: &mut H, bytes: &[u8]) { let n = bytes.len(); }\r\n";
    line.iter().copied().cycle().take(bytes).collect()
}

fn colored_ls(bytes: usize) -> Vec<u8> {
    let line = b"\x1b[0m\x1b[01;34mdirectory\x1b[0m  \x1b[01;32mexecutable\x1b[0m  \x1b[38;5;208mfile.rs\x1b[0m  \x1b[38;2;10;200;120mtruecolor\x1b[0m\r\n";
    line.iter().copied().cycle().take(bytes).collect()
}

fn htop_like(bytes: usize) -> Vec<u8> {
    let mut v = Vec::new();
    let mut i = 0u32;
    while v.len() < bytes {
        v.extend(
            format!(
                "\x1b[{};{}H\x1b[7m CPU[{:>3}%] \x1b[27m\x1b[32m||||||||\x1b[0m",
                i % 40 + 1,
                (i * 7) % 60 + 1,
                i % 100
            )
            .bytes(),
        );
        i += 1;
    }
    v
}

fn vim_scroll(bytes: usize) -> Vec<u8> {
    let mut v = b"\x1b[?1049h\x1b[1;40r".to_vec();
    let mut i = 0u32;
    while v.len() < bytes {
        v.extend(
            format!(
                "\x1b[40;1H\n\x1b[38;5;{}mline {i} fn main() {{ println!(\"x\"); }}\x1b[0m\x1b[K",
                i % 255
            )
            .bytes(),
        );
        i += 1;
    }
    v
}

fn parse(c: &mut Criterion) {
    let size = 4 * 1024 * 1024;
    let streams: [(&str, Vec<u8>); 4] = [
        ("cat_source", source_code(size)),
        ("ls_color", colored_ls(size)),
        ("htop", htop_like(size)),
        ("vim_scroll", vim_scroll(size)),
    ];
    let mut group = c.benchmark_group("parse");
    group.throughput(Throughput::Bytes(size as u64));
    group.sample_size(10);
    for (name, data) in &streams {
        group.bench_function(*name, |b| {
            let mut term = Term::new(Config::default(), &Size(160, 48), VoidListener);
            let mut proc: Processor<StdSyncHandler> = Processor::new();
            let mut sniffer = OscSniffer::new();
            b.iter(|| {
                for chunk in data.chunks(64 * 1024) {
                    black_box(sniffer.feed(chunk));
                    proc.advance(&mut term, chunk);
                }
            });
        });
    }
    group.finish();
}

fn visible_copy(c: &mut Criterion) {
    let mut term = Term::new(Config::default(), &Size(200, 60), VoidListener);
    let mut proc: Processor<StdSyncHandler> = Processor::new();
    proc.advance(&mut term, &colored_ls(512 * 1024));
    c.bench_function("visible_cell_copy_200x60", |b| {
        b.iter(|| {
            let grid = term.grid();
            let mut n = 0usize;
            for l in 0..60 {
                let row = &grid[Line(l)];
                for col in 0..200 {
                    let cell = &row[Column(col)];
                    n = n.wrapping_add(cell.c as usize).wrapping_add(cell.flags.bits() as usize);
                }
            }
            black_box(n)
        })
    });
}

fn keys(c: &mut Criterion) {
    let inputs = [
        KeyInput { key: "a", key_char: Some("a"), mods: Mods::default(), alt_is_meta: true },
        KeyInput {
            key: "left",
            key_char: None,
            mods: Mods { alt: true, ..Mods::default() },
            alt_is_meta: true,
        },
        KeyInput {
            key: "c",
            key_char: Some("c"),
            mods: Mods { ctrl: true, ..Mods::default() },
            alt_is_meta: true,
        },
        KeyInput {
            key: "f5",
            key_char: None,
            mods: Mods { shift: true, ..Mods::default() },
            alt_is_meta: true,
        },
    ];
    c.bench_function("key_encode", |b| {
        b.iter(|| {
            for i in &inputs {
                black_box(encode(i, KeyModes::default()));
            }
        })
    });
}

criterion_group!(benches, parse, visible_copy, keys);
criterion_main!(benches);
