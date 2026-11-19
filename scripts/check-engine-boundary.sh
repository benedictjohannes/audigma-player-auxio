#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Audigma Player Project
#
# Fails when the engine module (everything under engine/) imports Auxio code, depends on the Auxio
# modules, or when a file we own lacks an SPDX header or still carries an Auxio one.
cd "$(dirname "$0")/.." || exit 1
fail=0

if grep -rnE '^import +(static +)?org\.oxycblt' engine/src; then
  echo "engine must not import Auxio packages" >&2; fail=1
fi
if grep -nE 'project\(":(app|musikr)"\)' engine/build.gradle; then
  echo "engine must not depend on :app or :musikr" >&2; fail=1
fi

for f in $(find engine scripts/build.sh scripts/build-glue.sh scripts/check-engine-boundary.sh \
    -type f \( -name '*.kt' -o -name '*.java' -o -name '*.rs' -o -name '*.sh' -o -name '*.gradle' \
    -o -name '*.xml' -o -name 'Cargo.toml' \) -not -path '*/build/*' -not -path '*/target/*'); do
  head -8 "$f" | grep -q 'SPDX-License-Identifier: GPL-3.0-or-later' \
    || { echo "missing SPDX header: $f" >&2; fail=1; }
  if head -3 "$f" | grep -qi 'Auxio'; then echo "header mentions Auxio: $f" >&2; fail=1; fi
done
exit $fail
