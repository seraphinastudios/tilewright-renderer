#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

temporary=$(mktemp -d /tmp/tilewright-renderer-dependency-test.XXXXXX)
trap 'rm -rf -- "${temporary}"' EXIT

mkdir "${temporary}/bin"
cat >"${temporary}/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
test "$*" = "metadata --locked --offline --all-features --format-version 1"
cat "${MOCK_CARGO_METADATA}"
EOF
chmod +x "${temporary}/bin/cargo"

run_check() {
  PATH="${temporary}/bin:${PATH}" \
    MOCK_CARGO_METADATA="$1" tools/check-dependencies.sh
}

updated="${repository}/tools/fixtures/dependency-metadata.json"
run_check "${updated}"

build_dependency="${temporary}/build-dependency.json"
jq '(.packages[] | select(.name == "tilewright-render-wgpu")
  | .dependencies[] | select(.name == "naga") | .kind) = "build"' \
  "${updated}" >"${build_dependency}"
run_check "${build_dependency}"

approved_parentheses="${temporary}/approved-parentheses.json"
jq '(.packages[] | select(.name == "transitive") | .license) =
  "(MIT OR GPL-3.0-only) AND Apache-2.0"' \
  "${updated}" >"${approved_parentheses}"
run_check "${approved_parentheses}"

unapproved_license="${temporary}/unapproved-license.json"
jq '(.packages[] | select(.name == "transitive") | .license) =
  "MIT AND GPL-3.0-only"' \
  "${updated}" >"${unapproved_license}"
if run_check "${unapproved_license}"; then
  echo "dependency policy accepted an unapproved graph license" >&2
  exit 1
fi

unapproved_exception="${temporary}/unapproved-exception.json"
jq '(.packages[] | select(.name == "transitive") | .license) =
  "Apache-2.0 WITH LLVM-exception"' \
  "${updated}" >"${unapproved_exception}"
if run_check "${unapproved_exception}"; then
  echo "dependency policy accepted an unapproved SPDX exception" >&2
  exit 1
fi

malformed_license="${temporary}/malformed-license.json"
jq '(.packages[] | select(.name == "transitive") | .license) =
  "GPL-3.0-only OR OR OR MIT@"' \
  "${updated}" >"${malformed_license}"
if run_check "${malformed_license}"; then
  echo "dependency policy accepted a malformed license expression" >&2
  exit 1
fi

unapproved_source="${temporary}/unapproved-source.json"
jq '(.packages[] | select(.name == "transitive") | .source) =
  "git+https://example.invalid/transitive?rev=0123456789abcdef#0123456789abcdef"' \
  "${updated}" >"${unapproved_source}"
if run_check "${unapproved_source}"; then
  echo "dependency policy accepted an unapproved graph source" >&2
  exit 1
fi

undeclared_direct="${temporary}/undeclared-direct.json"
jq '(.packages[] | select(.name == "tilewright-render") | .dependencies) +=
  [{"name":"transitive","source":"registry+https://github.com/rust-lang/crates.io-index","kind":null}]' \
  "${updated}" >"${undeclared_direct}"
if run_check "${undeclared_direct}"; then
  echo "dependency policy accepted an undeclared direct dependency" >&2
  exit 1
fi
