# v0024 start

No layer files: vigild starts with an empty plan; the test then points
the layers symlink to app/, broken/ (app.yaml plus a broken layer) and
app/ again, and --watch-layers picks each change up.
