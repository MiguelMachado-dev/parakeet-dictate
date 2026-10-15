#!/usr/bin/env bash
# Prints tables.md: every table docs/perf/README.md quotes, in full, under the command that prints it.
#   docs/perf/tables.sh > docs/perf/tables.md
# Set PY to run the harness with another interpreter than `uv run python`.
cd "$(dirname "$0")/../.." || exit 1
PY=${PY:-uv run python}
D=docs/perf/data

table() {
    printf '\n```\npython scripts/bench_resources.py %s\n```\n\n' "$*"
    $PY scripts/bench_resources.py "$@" || exit 1
}

echo "# Tables"
echo
echo "Cells read \"median [min-max] p95 X, n Y\". Memory and time-to-ready metrics take one value per process;"
echo "latencies pool every call of a side. See README.md for what each metric measures."

echo
echo "## Model on CUDA, da3c189 against the final tree"
table compare $D/stages/model-cuda $D/stages/model-cuda --before-side base --after-side final

echo
echo "## Model on CUDA, medians after each commit"
table summary $D/stages/model-cuda --markdown

echo
echo "## Model on CUDA, each commit against its parent"
for pair in base:c1 c1:c2 c2:c3 c3:final; do
    table compare $D/stages/model-cuda $D/stages/model-cuda --before-side "${pair%:*}" --after-side "${pair#*:}"
done

echo
echo "## Model on CUDA, the final tree without the high QoS tag"
table compare $D/stages/model-cuda-tag $D/stages/model-cuda-tag --before-side notag --after-side final

echo
echo "## Model on the CPU provider, 35fb1f7 with and without session.force_spinning_stop"
table compare $D/stages/model-cpu-spinning $D/stages/model-cpu-spinning --before-side final --after-side stopspin

echo
echo "## Model on the CPU provider, medians of da3c189, 35fb1f7 without the tag, and 35fb1f7"
table summary $D/stages/model-cpu-reps3 --markdown

echo
echo "## Model on the CPU provider, one round of clips, da3c189 against 35fb1f7"
table compare $D/stages/model-cpu $D/stages/model-cpu --before-side base --after-side final

echo
echo "## Settings, da3c189 against the final tree"
table compare $D/stages/settings $D/stages/settings --before-side base --after-side final

echo
echo "## Settings, medians after 3998e3a and after the final tree"
table summary $D/stages/settings --markdown

echo
echo "## Overlay rendering, da3c189 against the final tree"
table compare $D/stages/overlay $D/stages/overlay --before-side base --after-side final

echo
echo "## da3c189 in the baseline sessions against da3c189 in the stages sessions"
for kind in model-cuda model-cpu settings overlay; do
    table compare $D/baseline/$kind $D/stages/$kind --after-side base
done
