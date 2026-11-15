# Time-stretch study

*Milestone 5, step 1. Written 2026-11-11. Nothing here is decided yet; the choice goes in DECISIONS.md once it is made.*

Speed and pitch changes (milestone 5) and the slow-down practice loop (milestone 7) both need a time-stretcher: something that changes how long audio lasts without changing its pitch, or the pitch without the length. This document explains how such algorithms work, then how each candidate library does it, how they compare with the modulator in audio-tools (our own earlier Python work), and what that means for soundglass.

## 1. The problem

Before this study, Playback (now Track) → Change speed resamples: the audio is read faster or slower, like a tape. Every frequency is scaled by the same factor, so 1.25× is also 1.25× higher in pitch (almost 4 semitones). That is correct for varispeed, but not what a musician practising a solo wants.

Time-stretching keeps each sound's frequencies while changing how long it lasts. Pitch-shifting is the reverse, and is usually built from the same machine: stretch by a factor, then resample by its inverse, and the length comes back while the pitch has moved.

Speech is the easy case: one voice, one pitch at a time. Music is harder for two reasons:
- **Many sources at once.** A chord or a band has many pitches overlapping, and a method that locks onto "the" pitch period breaks down.
- **Transients.** A drum hit or a plucked note's attack is short and sharp. Stretching smears or doubles it, and the ear notices that before anything else.

## 2. The building blocks

### OLA — overlap-add
Cut the input into short overlapping windows (say 50 ms, with a smooth fade-in and fade-out shape) and lay them down again with a different spacing: closer together to speed up, further apart to slow down. The windows overlap and add into the output.

It fails because the windows' waveforms don't line up where they overlap: a sine in one window meets the same sine shifted by some random fraction of a cycle in the next, and they partly cancel. It sounds rough and warbly.

### SOLA and WSOLA — overlap-add, but aligned
Before laying each window down, slide it a little (within a few milliseconds) to the position where it best matches what is already in the output, measured by cross-correlation. The waveforms now line up, so they add instead of cancelling. WSOLA (Verhelst & Roelands, 1993) does the search on the input side, picking which piece of the input to take next so it continues the previous one most naturally.

These are *time-domain* methods: cheap, no FFT, and very good for speech and for single instruments. On dense music the best alignment for one instrument is wrong for another, so some part of the mix always stutters. Transients can be repeated (when slowing down, the same attack lands in two windows) or dropped (when speeding up).

### PSOLA — pitch-synchronous overlap-add
PSOLA (Moulines & Charpentier, 1990) first finds the pitch period, then places windows exactly one period apart, centred on each pitch pulse. Moving the windows closer or further apart changes the pitch, and repeating or dropping whole periods changes the length. For a single voice it is excellent and keeps the formants (the voice's timbre) naturally. It needs one clear pitch, so it can't handle chords or full mixes.

### The phase vocoder — the frequency-domain approach
The phase vocoder (Flanagan & Golden, 1966; Dolson's 1986 tutorial made it widely known) works on the spectrum instead of the waveform:

1. **Analysis.** A short-time Fourier transform (STFT): overlapping windows, each turned by an FFT into bins. Each bin has a magnitude (how loud that frequency is) and a phase (where in its cycle it is).
2. **Modification.** The output frames are spaced by a different hop than the input frames. Magnitudes are kept. Phases must be moved on: from one input frame to the next, each bin's phase advanced by some amount. That amount tells you the bin's exact frequency. In the output, the phase is advanced by that frequency × the *output* hop, so each sinusoid keeps running smoothly at its own frequency.
3. **Synthesis.** An inverse FFT of each modified frame, windowed and overlap-added.

Because each frequency is handled on its own, chords and mixes work. The classic failure is **phasiness**: each bin's phase is moved on independently, so bins that belonged to one sinusoid drift apart from each other. The sound becomes smeared and reverberant, as if heard from further away. Transients are smeared too, since a window is tens of milliseconds long.

### Phase locking — keeping bins together
A single sinusoid spreads over several neighbouring bins, and their phases are related. Laroche & Dolson (1999) found the fix: find the peaks in each frame's spectrum, advance only each peak's phase the normal way, and set the bins around it so their phase relationship to the peak stays as it was in the input.
- **Identity phase locking:** the bins around a peak keep exactly their input phase offset from the peak.
- **Scaled phase locking:** the same, with a correction when a peak moves from one bin to another between frames.

This is also called keeping **vertical phase coherence** (across frequency, within a frame), as opposed to **horizontal phase coherence** (across time, within a bin), which the basic phase vocoder keeps. Good stretchers balance both. Phase locking removes most of the phasiness.

### Transients
The main approaches:
- **Phase reset.** Detect onsets (for example a sudden rise in spectral energy, "spectral flux"). At an onset, copy the input phases straight into the output instead of advancing them, so the attack is reproduced as it was.
- **Move the stretching away from transients.** Stretch less (or not at all) right at an onset, and more in the steady parts between, so the overall factor is still right.
- **Multi-resolution.** Long windows resolve close frequencies well (good for bass and sustained tones) but blur time. Short windows keep attacks sharp but blur frequency. Some stretchers analyse with several window lengths at once and use each where it fits.

### Phase reconstruction — Griffin–Lim
A different way round the phase problem: keep only the magnitudes, change them as needed, and then *invent* phases that fit. Griffin & Lim (1984) showed how:
1. Start from some phase, often random.
2. Turn the spectrogram (wanted magnitudes plus current phases) back into a signal with an inverse STFT.
3. Take that signal's STFT. Its magnitudes are not quite the wanted ones, because overlapping frames constrain each other, but its phases are now consistent with a real signal.
4. Keep those phases, put the wanted magnitudes back, and go to 2.

Each round brings the result closer to a signal whose STFT really has the wanted magnitudes. "Fast Griffin–Lim" (Perraudin, Balazs & Søndergaard, 2013) adds momentum, pushing each step a little further in the direction it is already going, and gets there in far fewer rounds.

Since no input phase is carried over, nothing can drift apart the way a basic phase vocoder's bins do. But the phases it finds are only "consistent", not the original ones, and that shows:
- a slightly metallic, phasey sound on tones;
- attacks smeared, often with a faint pre-echo, since nothing tells it where the attack was;
- a low noise floor when started from random phases;
- a different result on each run.

It needs the whole spectrogram and many passes over it, so it works offline. Real-time variants exist: RTISI and RTISI-LA (Zhu, Beauregard & Wyse, 2007) build the phases frame by frame with a few frames of look-ahead.

### Pitch shifting and formants
Two ways:
- **Stretch then resample.** Stretch by the pitch factor, then resample back to the original length. Simple, and any stretcher can do it.
- **Move the bins.** In the frequency domain, move each bin's energy to a new frequency directly, with no resampling.

Either way, shifting a voice up moves its **formants** too. Formants are the resonances of the throat and mouth that give a voice its character, and they stay fixed in real speech. Moving them makes the "chipmunk" effect. Formant preservation estimates the spectral envelope, shifts the pitch, and puts the original envelope back.

## 3. The candidates

### Signalsmith Stretch
MIT licence, header-only C++11. Its author explains the design in a blog post (2023), building on his ADC22 talk *Four Ways To Write A Pitch-Shifter*.

**Algorithm.** It is a phase vocoder that rethinks the phase step as **prediction from nearby points**:
- Each output magnitude comes from the input spectrum at the matching point of the time/frequency map.
- Each output phase is predicted from points already decided. The phase change between two input points is measured as X[p₂]·conj(X[p₁]) and applied to an output point already set. The aim, in the author's words, is to "take measurements of the relative phase in the input compared to nearby points, and attempt to create similar phase relationships in the output".
- It works in two stages:
  - First, a *horizontal* prediction from the previous output frame, which is the normal phase vocoder step.
  - Then a second pass blends in up to four *vertical* predictions from the bins below and above (one bin and a wider step each way), using the first-stage values.
- Predictions are combined by adding the complex numbers, not by averaging angles. Each carries its magnitude, so loud tones and transients naturally weigh more, and no trigonometry is needed.
- This is phase locking in spirit, done as a weighted vote rather than by finding peaks.
- For stereo, each bin is predicted on the loudest channel, and the other channels copy the input's phase difference to it. That keeps the stereo image.

**Transients.** There is no transient detector. Beyond 2× stretch it partly randomises phases (`maxCleanStretch`), which the author calls "a juddery smudge… It's still wrong, but it's a more familiar type of wrong". The README says it "sounds best for more modest changes (between 0.75x and 1.5x)".

**Pitch.** It moves bins in the frequency domain, not by resampling. It is unusual in doing time and pitch separately.
- It finds spectral peaks and builds a frequency map that is locally 1:1 around strong harmonics, which avoids frequency aliasing.
- A *tonality limit* shifts frequencies only up to a corner frequency and leaves those above it unshifted.
- Formants can be preserved or shifted on their own (`setFormantFactor`, `setFormantSemitones`). The author says this is less sharp than monophonic methods like PSOLA.

**Live use.**
- The default preset uses 120 ms blocks with a 30 ms interval.
- The ratio is simply the input length ÷ output length passed to each `process()` call, so it can change on every call.
- Buffers are sized in `configure()`. Reading the code, `process()` doesn't reallocate, but this isn't promised. Its peak list grows with `emplace_back` after a `reserve`, so it is *suspected* safe but needs a check.
- It sometimes does a burst of work at once. `splitComputation` spreads that out at the cost of one interval of extra latency.
- The latency in samples is not stated, only that `inputLatency()` and `outputLatency()` report it.

**Build and binding.** It builds as C++ through `cc`, as we already do for LAME and libvorbis. The Rust wrappers are `signalsmith-stretch` 0.1.3 and `ssstretch` 0.1.0 (cxx-based), both small and young. A thin binding of our own is also possible: the API is a handful of methods.

### timestretch (Rust)
MIT licence, pure safe Rust (`forbid(unsafe)`), FFT through rustfft. Version 0.16.0 is from 2026-10-29. It is built for DJ use: "keylock" (tempo without pitch) and "tape" (pitch follows tempo).

**Algorithm.** It has changed a great deal between versions, according to its changelog:
- **Up to 0.6:** a phase vocoder stream processor, an optional three-band (Linkwitz–Riley) multi-resolution engine, and an offline hybrid of WSOLA and phase vocoder that switched at transients. A "varispeed first" path resampled for tempo, then corrected the pitch with the phase vocoder.
- **0.7:** a two-band keylock. Below 150 Hz the audio is left alone, and above it a beat-synchronous SOLA corrects pitch for DJ-sized changes (up to about 9 %). Larger changes used a small-FFT phase vocoder with identity phase locking, fading to plain varispeed beyond 12–22 %. Transients came from a pre-analysis, or from spectral flux live.
- **0.8:** WSOLA, the hybrid and the phase vocoder corrector were removed from the live path, leaving SOLA alone.
- **0.11–0.13:** a "wide keylock" for 0.25–2×. It is a full-spectrum, FFT-2048, identity-locked phase vocoder with phase resets per band, and mid/side for stereo. Then a period-aligned SOLA corrector was added for the band below 120 Hz.
- **0.16:** everything was merged into "one production processor". The design document is left out of the published crate, so **which algorithm 0.16 runs is unverified**. It is probably the identity-locked phase vocoder plus SOLA bass corrector from 0.11–0.13, as its changelog keeps "reference kernels" unchanged.

**Pitch.** Pitch ratio 0.25–4 (±24 semitones). 0.16 doesn't say how; up to 0.8 it was phase-vocoder based. Formants and more than two channels are "outside this processor's scope".

**Live use.** This is its strength. "Preparation allocates plans and bounded storage. Processing performs no allocation or locking." Parameters are scheduled and reported with the output position where they took effect. Latency was about 13 ms for keylock in earlier versions; 0.16 reports it per stage instead.

**Build and binding.** It is Rust, so there's no binding to write, and the dependencies are light. The risks:
- The API changes a lot between versions.
- It is tuned for electronic and DJ material: its own notes add "non-EDM" test material only from 0.12.
- "Human listening remains a separate result", so its quality on acoustic music is unknown.

### Bungee
MPL-2.0 licence, C++20. It needs CMake 3.30 or later and the Eigen and PFFFT submodules. A paid "Bungee Pro" shares the API.

**Algorithm.** A grain-based phase vocoder, "typically processing around 100 grains per second".
- The FFT is 8 times the synthesis hop. The hop is worked out in code not read in this study; at 44.1 kHz that suggests (*inferred*) a hop of about 512 and an FFT of about 4096.
- Each bin's magnitude and phase are taken from the channels summed together, so there is one phase per bin for all channels.
- Bins are grouped into **partials** by energy. The phase advance is measured once at each partial's peak and added to every bin of that partial. That is identity phase locking in practice, though the code doesn't use the name.
- Transients: partials whose energy jumps from the previous grain are suppressed (`suppressTransientPartials`). Bungee Pro advertises "novel processing techniques that deliver crisp transients", which suggests transients are where the free version is weaker.
- The website calls it "an adaptive phase vocoder" without saying what adapts.

**Pitch.** By resampling around the stretch, before or after it depending on a per-grain mode (*inferred from the code*). Input and output sample rates can differ, so it converts the rate in the same step.

**Live use.**
- Speed and pitch are set per grain, and the speed comes from how far each grain's position moves.
- The granular API allows any movement: any speed, zero, reverse, scrubbing. That fits a practice tool well. The streaming API is forward only.
- Buffers are sized in the constructor, and debug builds forbid Eigen from allocating at run time, which suggests no allocation while processing. That is not stated as a guarantee.
- The pipeline is 4 grains deep, so the latency is a few hops (*inferred*).

**Build and binding.** CMake would be a new build tool for us. The Rust binding `bungee-rs` is small: 0.2.0 was yanked, docs.rs fails to build both versions, and it seems to follow an older Bungee API.

### Rubber Band
GPL (or commercial). GPL-2.0-or-later is compatible with our GPL-3.0-or-later. It is the most widely used library of its kind, found in Audacity, Ardour and many others.

**Algorithm.** Two engines:
- **R2** ("Faster"): a phase vocoder with transient detection. The integration guide says the stretch rate varies locally around detected percussive transients. Its "crisp" transient option resets phases at transients, according to the library's option documentation (*not re-read for this study*).
- **R3** ("Finer", from version 3.0, 2022): "generally outputs higher quality than the older R2 engine at greater CPU cost". It analyses with several window lengths (a "multi-window" mode), which its short-window option turns off. Exactly how it combines them isn't publicly documented; descriptions of long, mid and short FFT bands come from third parties and are *unverified*.

**Pitch.** Stretch then resample, with formant preservation available.

**Live use.** It has a real-time mode. Its documented start delays at 44.1/48 kHz are 512 samples for R2 short, 1280 for R3 short and 2048 for R3 default.

**Build and binding.** There is no maintained Rust crate. We would write FFI to its C API and build its C++ ourselves, the most work of all the candidates.

### Our own WSOLA
About 150 lines of Rust, no dependency, fully understood and easy to make allocation-free. It suits speech and single instruments, and mild slow-downs (0.75× and up) on light material. On a full band it will warble and repeat attacks, as described in section 2. Pitch would be stretch then resample, which we can already do with rubato.

### The audio-tools modulator (ours, Python)
The speed and pitch modulator in `audio-tools/` (`speed_modulator.py` and `pitch_modulator.py`, built on librosa). It is a fifth family next to the four above: **magnitude interpolation plus phase reconstruction**.

**Algorithm.**
1. Load the file with `librosa.load`. With no arguments this resamples to 22 050 Hz mono (*librosa's documented default; librosa isn't installed here to confirm*).
2. Take the STFT and keep only the magnitudes; the input phase is thrown away.
   - Speed uses n_fft 2048 (about 93 ms at 22.05 kHz) with a 512 hop.
   - Pitch uses n_fft 4096 with a 1024 hop.
3. Change the magnitudes:
   - **Speed:** each frequency bin's magnitude over time is redrawn with a cubic spline at `ceil(frames ÷ factor)` evenly spaced points. A factor of 0.7 gives 1/0.7 as many frames, so the audio lasts longer.
   - **Pitch:** each frame's spectrum is redrawn along frequency with a cubic spline, so the value at frequency f is the original's at f ÷ 2^(n/12). The whole spectrum is moved by n semitones and the number of frames stays the same.
4. Rebuild a signal with `librosa.griffinlim` at the same hop. Its defaults are 32 rounds of fast Griffin–Lim (momentum 0.99) from random phases (*also librosa's documented defaults, unconfirmed here*).
5. Optionally add count-in clicks at the song's tempo (`utils_audio.add_intro_ticks`, using `librosa.beat.beat_track`), then write MP3 through ffmpeg.

`speed_modulator_librosa.ipynb` runs librosa's own `effects.time_stretch` on the same song for comparison. That is a basic phase vocoder.

**What follows from the design.**
- **Phase:** it never moves phases on. All coherence, vertical and horizontal, comes from Griffin–Lim making the STFT consistent. So it avoids the basic phase vocoder's drifting bins but trades them for Griffin–Lim's own sound (section 2).
- **Time:** interpolating the magnitudes means no frames are skipped or repeated, as they are in OLA and WSOLA. Slow-downs come out smooth on sustained notes. But an attack that rose within one frame now rises over 1 ÷ factor frames. With nothing to detect transients and no phase to keep them sharp, attacks soften, more so the slower it goes.
- **Pitch:** moving the bins is the same idea Signalsmith Stretch uses, but on the whole spectrum linearly. There is no peak-aware map that keeps harmonics 1:1, and no formant handling, so voices take on the chipmunk or giant sound.
- **Cost:** offline only. The whole song, then 32 rounds of STFT plus inverse STFT. That is fine on a 3-minute song at 22 kHz mono, and about four times the work at 44.1 kHz stereo.

### The audio-tools modulator against each candidate
- **Basic phase vocoder** (librosa's `time_stretch`, the one you compared it with):
  - It uses the same STFT, but keeps the input phase and moves it on, where ours rebuilds it.
  - The phase vocoder keeps the original phases where they matter (steady tones, the start of attacks) but drifts into phasiness. Ours never drifts but never had the original phases.
  - Expect the phase vocoder to be more "present" on tones and ours to be smoother but a little metallic.
  - Both are offline as written.
- **Signalsmith Stretch:**
  - It is the closest relative. Both work on the spectrum, and both move bins for pitch.
  - Where ours iterates 32 times to find consistent phases, Signalsmith *predicts* them in one pass from neighbouring points, using the input's own phase relationships. It gets coherence by construction instead of by search, and works block by block, live.
  - For pitch, Signalsmith keeps strong harmonics 1:1, has a tonality limit and keeps formants; ours scales everything evenly.
  - Neither detects transients.
- **timestretch:**
  - The opposite approach: mostly time-domain (SOLA) plus a locked phase vocoder for wide ratios.
  - It is built to run live with no allocation. Ours can't run live at all.
  - Its focus is DJ tempo changes; ours was made for practice slow-downs (0.66–0.7× in the scripts).
- **Bungee:**
  - It keeps each tone's bins together by locking them to the partial's peak: phase locking, so it keeps much more of the original phase than ours.
  - Both soften attacks, Bungee on purpose (it suppresses partials whose energy jumps), ours as a side effect of interpolating.
  - Bungee's grains can go anywhere, reverse included; ours can only stretch a whole file.
- **Rubber Band:**
  - It is furthest from ours on attacks. It detects transients, resets phase there and moves the stretch away from them, and R3 analyses with several window lengths.
  - On drums and plucked notes it should sound clearly sharper. On sustained acoustic music the gap is smaller.
- **WSOLA:**
  - Time domain, cheap and live, with opposite failures: WSOLA stutters and doubles attacks on dense music, ours smears them.
  - WSOLA keeps the true waveform (and true phase) inside each window, which ours never has.

### What is specific to the current scripts
These are details of the scripts, not limits of the method:
- **22.05 kHz mono.** Loading at librosa's default rate drops everything above about 11 kHz (cymbals, breath, air) and the stereo image.
- **Spline overshoot.** Cubic splines can swing past the points they join, so around sharp changes the new magnitudes can go negative. Griffin–Lim then treats a negative magnitude as a phase flip.
- **Pitch down extrapolates.** For a downward shift, the top of the new spectrum asks the spline for values beyond the original's highest frequency, where a cubic spline extrapolates freely.
- **Random start.** Griffin–Lim from random phases gives a slightly different result each time and a faint noise floor.

### How it could be improved (ideas, not plans)
- **Start Griffin–Lim from phase-vocoder phases** instead of random ones. Starting closer to the answer is known to need far fewer rounds and to keep attacks sharper.
- **Interpolate log-magnitudes** (or use a spline that can't overshoot), and clamp at 0.
- **Keep the native rate and stereo.** Rebuild mid and side, or left and right with shared phases, as Signalsmith copies phase differences between channels.
- **RTISI-LA** for frame-by-frame, live use.
- **Porting it to Rust** would need only an FFT, and realfft and rustfft are already in our tree through rubato.

### Smaller crates seen
- `wsola` 0.1.0: MIT, pure Rust WSOLA.
- `pitch_shift` 2.1.0: a phase vocoder.
- `phase_vocoder` 0.1.0, whose own README says it is "very immature".
- `rocoder`, which says it is "fairly naive, and probably not quite correct".

None is better than writing our own WSOLA or picking one of the four above.

## 4. What it means for soundglass

What we need:
- **Speed and pitch as edits (milestone 5).** These are rendered once, not live, so the best quality matters more than CPU. A finished stretch can't be described as pieces of the original, so how it fits the edit list is a question for the next plan.
- **The slow-down practice loop (milestone 7).** It runs inside the audio callback, usually between 0.5× and 1×, with the speed changeable while it plays. That needs:
  - block-by-block processing
  - no allocation in the callback
  - low latency
  - no clicks when the ratio changes
- **Builds on Linux and Windows,** with nothing new to install if possible.

How the candidates compare on those needs:
- **Signalsmith Stretch** has the most thought-through algorithm here and the best pitch and formant controls. Its build fits what we already do. Its weak spot is strong slow-downs: below 0.75× it is outside the range its author recommends, which matters for a practice loop at 0.5×.
- **timestretch** is the only pure-Rust choice, with the clearest real-time promises. But its current algorithm isn't documented, its API moves fast, and it was tuned for electronic music; for acoustic music it would have to prove itself by ear.
- **Bungee** has the most flexible live control (reverse, scrubbing, any speed), and a phase-locked phase vocoder. Against it: a new build tool (CMake), a fragile Rust binding, and transients kept for the paid version.
- **Rubber Band** probably has the best quality and the best transient handling, but it is the most work to bind and build.
- **Our own WSOLA** is the cheapest and safest, and probably enough for practising a single instrument. It is clearly worse on full mixes.

- **The audio-tools modulator** is ours and understood. Ported to Rust it would need nothing new. As it stands it can only make offline edits, and it would need fixes (native rate, stereo, a better start for Griffin–Lim) to match the others there. It can't be the live loop without RTISI-LA. Its count-in clicks fit milestone 7's metronome.

The two needs might be met by one engine or two (for example, the best offline engine for edits and a lighter one for the live loop). One engine is simpler and makes edits sound the same as the loop.

**Decision (2026-11-11):** our own WSOLA now, for both, as the simplest true time-stretch; speed and pitch are live playback settings that Save As can apply, not edits. The audio-tools modulator comes back in milestone 10, ported to Rust and improved, with a Fast (real-time) and a Quality (offline) setting.

## 5. Unverified

- Which algorithm timestretch 0.16 runs (its design document isn't in the published crate).
- Bungee's exact hop, window shape and transient logic (`Timing.cpp`, `Window.cpp` and `Partials.cpp` were not read).
- Signalsmith Stretch's latency in samples, and whether `process()` really never allocates.
- The internals of Rubber Band R3, and R2's phase reset (from its option documentation, not re-read).
- librosa's defaults as the audio-tools scripts use them (22 050 Hz mono load; 32 rounds of fast Griffin–Lim from random phases): from librosa's documentation, not checked against an installed librosa.
- Quality on our material for every candidate, the audio-tools modulator included: nothing in this study has been listened to.

## 6. Next step (optional)

A benchmark and blind listening test in a throwaway crate outside the repo:
- **Clips:** two 30 s clips from the test album (trumpet and voice).
- **Settings:** 0.5×, 0.75× and 1.25× with the pitch kept; −2 and +3 semitones with the length kept.
- **Measurements:**
  - speed (how many times faster than real time)
  - allocations inside the process call, with a counting allocator
  - output length against the target
  - the pitch of a test sine
- **Blind files:** written with shuffled names and a separate key, for you to rank by ear.
- **The audio-tools modulator in the same set,** run on the same clips (as it is, and at 44.1 kHz stereo), so it is judged blind next to the libraries.

## 7. Sources

- Signalsmith Stretch: [project page](https://signalsmith-audio.co.uk/code/stretch/), [design post (2023)](https://signalsmith-audio.co.uk/writing/2023/stretch-design/), [repository](https://github.com/Signalsmith-Audio/signalsmith-stretch), [header source](https://raw.githubusercontent.com/Signalsmith-Audio/signalsmith-stretch/refs/heads/main/signalsmith-stretch.h)
- Signalsmith Rust wrappers: [signalsmith-stretch](https://docs.rs/signalsmith-stretch), [ssstretch](https://docs.rs/ssstretch)
- timestretch: [docs.rs README](https://docs.rs/crate/timestretch/latest), [CHANGELOG](https://docs.rs/crate/timestretch/latest/source/CHANGELOG.md), [Cargo.toml](https://docs.rs/crate/timestretch/latest/source/Cargo.toml), [timestretch-core](https://docs.rs/timestretch-core/latest/timestretch_core/), [repository](https://github.com/robmorgan/timestretch-rs)
- Bungee: [repository](https://github.com/kupix/bungee), [website](https://bungee.parabolaresearch.com/), [bungee-rs](https://docs.rs/bungee-rs)
- Rubber Band: [R3 announcement (2022)](https://breakfastquay.com/news/20220707.html), [integration guide](https://www.breakfastquay.com/rubberband/integration.html)
- Small crates: [wsola](https://docs.rs/crate/wsola/0.1.0), [pitch_shift](https://docs.rs/crate/pitch_shift/2.1.0), [phase_vocoder](https://docs.rs/crate/phase_vocoder/0.1.0), [rocoder](https://docs.rs/crate/rocoder/latest)
- The audio-tools modulator: `audio-tools/speed_modulator.py`, `audio-tools/pitch_modulator.py`, `audio-tools/speed_modulator_librosa.ipynb`, `audio-tools/utils_audio.py` (read for this study)
- Background papers, cited from general knowledge rather than re-read: Flanagan & Golden, "Phase vocoder" (1966); Dolson, "The phase vocoder: a tutorial" (1986); Moulines & Charpentier, PSOLA (1990); Verhelst & Roelands, WSOLA (1993); Laroche & Dolson, "Improved phase vocoder time-scale modification of audio" (1999); Griffin & Lim, "Signal estimation from modified short-time Fourier transform" (1984); Perraudin, Balazs & Søndergaard, "A fast Griffin-Lim algorithm" (2013); Zhu, Beauregard & Wyse, "Real-time signal estimation from modified short-time Fourier transform magnitude spectra" (RTISI-LA, 2007).
