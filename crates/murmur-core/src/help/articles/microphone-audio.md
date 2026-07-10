# Microphone and audio

Murmur captures your microphone, detects speech with voice activity detection,
and trims silence so only your words are transcribed. Most audio settings live in
Settings and take effect on your next recording.

## Choose an input device

Pick your microphone under Audio Device in Settings, or leave it on System
Default. If you switch headsets or plug in a new mic, set it here so Murmur
captures from the right input.

## Mic sensitivity

The Mic Sensitivity slider controls how readily Murmur treats sound as speech.
Higher picks up quieter speech but more background noise; lower ignores more noise
but may miss soft speech. The default is sensitive enough for normal speaking
without raising your voice.

## Noise floor calibration

By default Murmur calibrates its silence threshold from your ambient noise at the
start of each session, so it adapts to a quiet room or a noisy cafe automatically.
You do not need to set a fixed level. The auto threshold keeps soft speech in
while filtering steady background hum.

## Phrase pause and timeouts

Phrase Pause is the silence that ends one phrase so it gets delivered (0.6
seconds by default). Session Timeout is the total inactivity before the whole
session stops (60 seconds by default; set 0 for hands-free). A shorter silence
after you finish a phrase also auto-stops a session once you have spoken.

## Echo cancellation

Echo cancellation uses the OS voice-capture path to keep your microphone from
picking up your own speakers, which helps on calls. It is Windows only and falls
back to the raw microphone elsewhere. Turn it off in Settings if you do not need
it; transcription quality is otherwise unaffected.

## The silence fallback

On some audio setups the Windows voice-processing path hands back digital silence.
Murmur detects this on the first recording and automatically falls back to your
raw microphone so you are never left with a dead session. Turning echo
cancellation off in Settings uses the plain microphone directly.

## Sound cues

Sound Cues play a short chime when recording starts and stops, so you know when
Murmur is listening without watching the pill. Turn them off in Settings if you
prefer silent operation.
