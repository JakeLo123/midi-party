//! The audio callback must never allocate or free. This test counts every
//! allocation while `Engine::process` runs 1000 blocks and demands zero.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use mp_audio::demo::demo_song;
use mp_audio::{create_engine, load_soundfont, SoundFontSynth};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREES.fetch_add(1, Ordering::SeqCst);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn process_does_not_allocate() {
    let sample_rate = 48_000;
    let sf = load_soundfont(common::test_soundfont()).unwrap();
    let (mut engine, mut handle) = create_engine(sample_rate, false);
    handle.set_synth(0, Box::new(SoundFontSynth::new(&sf, sample_rate).unwrap())).unwrap();
    handle.set_synth(1, Box::new(SoundFontSynth::new(&sf, sample_rate).unwrap())).unwrap();
    handle.set_song(demo_song());
    handle.play().unwrap();
    handle.audition(0, 72, 100, 0.5).unwrap();
    let mut out = vec![0.0f32; 2 * 512];

    // Warm up: first callbacks may touch lazily-initialized thread locals.
    for _ in 0..20 {
        engine.process(&mut out);
    }
    handle.collect_garbage();

    let a0 = ALLOCS.load(Ordering::SeqCst);
    let f0 = FREES.load(Ordering::SeqCst);
    let mut heard_sound = false;
    for _ in 0..1000 {
        engine.process(&mut out);
        heard_sound |= out.iter().any(|s| *s != 0.0);
    }
    let allocs = ALLOCS.load(Ordering::SeqCst) - a0;
    let frees = FREES.load(Ordering::SeqCst) - f0;
    assert_eq!((allocs, frees), (0, 0), "Engine::process allocated {allocs} times and freed {frees} times");
    assert!(heard_sound, "expected audible output while playing");
}
