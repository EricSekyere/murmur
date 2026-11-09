# Local correction examples

In recent history, choose **Save mistake**, edit **What you actually said**,
then choose **Save audio and correction locally**. This saves only that selected
clip and text under Murmur's config directory, in `correction-examples`.
Nothing is uploaded, and saving an example does not create a replacement rule
or train a model. A separate explicit action is needed for every example.

The control is available only while audio remains in the existing RAM cache
(at most ten recent utterances and roughly sixty seconds total). Restarting,
clearing history, disabling history, or cache eviction can remove unsaved audio.
It cannot recover a phrase that was rejected before a history entry was made.

**Saved examples** shows the collection and its directory. **Delete audio and
text** removes one saved pair. Clearing history or disabling history does not
delete explicitly saved examples. Each folder contains a float WAV and a JSON
manifest usable directly by the stress runner. Files remain local user data;
do not commit personal recordings or reports.

The retained clip has already passed microphone capture, echo cancellation and
phrase splitting; it is not untouched microphone audio. `recognized` is the
saved delivered text and can include cleanup. `model_at_save` is the selected
model when the example is saved, not proof of which model produced the history
entry. The audio and corrected reference are the inputs for a new comparison.

For a single example, point `MURMUR_STRESS_MANIFEST` at its `manifest.json` and
choose the installed model using the variables in
[the evaluation guide](transcription-accuracy-evaluation.md). The optional Qwen
trial accepts the same manifest. Keep a separate held-out set while tuning.
Include ordinary speech, pauses, negations, numbers, actual background noise,
and varied speaking pace instead of concentrating on product names.
