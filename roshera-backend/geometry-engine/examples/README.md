# Kernel Demo Harness

Each `demo_*.rs` exercises one operation category and writes binary STLs
plus a manifest that the frontend's Demo Gallery (`/#/demos`) reads.

## Run a single demo

```bash
cargo run --release --example demo_primitives -p geometry-engine
```

Outputs land in `target/demos/<category>/*.stl` and append to
`target/demos/manifest.json`.

## Refresh the frontend Demo Gallery

The gallery loads STLs and the manifest from `roshera-app/public/demos/`.
Set `ROSHERA_DEMO_OUT` to point demo output straight at the public dir,
then run each demo from the `roshera-backend/` directory:

```bash
# bash / git-bash
export ROSHERA_DEMO_OUT="../roshera-app/public/demos"
for demo in demo_primitives demo_booleans demo_extrude_revolve \
            demo_sweep_loft demo_features demo_transforms \
            demo_pattern_draft; do
  cargo run --release --example "$demo" -p geometry-engine
done
```

```powershell
# PowerShell
$env:ROSHERA_DEMO_OUT = "..\roshera-app\public\demos"
foreach ($d in 'demo_primitives','demo_booleans','demo_extrude_revolve',
               'demo_sweep_loft','demo_features','demo_transforms',
               'demo_pattern_draft') {
  cargo run --release --example $d -p geometry-engine
}
```

After the demos finish, `roshera-app/public/demos/manifest.json` lists
every result and the gallery picks them up on next refresh.

## CI regression role

`.github/workflows/ci.yml` runs the same set of demos on every push.
The demos assert non-zero triangle counts and bbox invariants, so a
regression in primitives, booleans, sweep, loft, features, transforms,
or pattern/draft fails CI loudly. See `plans/steady-sauteeing-nova.md`
Phase 3 for the full design.
