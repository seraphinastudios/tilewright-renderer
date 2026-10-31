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

# Font data is permitted for precisely the reviewed UI package/version.
approved_font="${temporary}/approved-font.json"
jq '(.packages[] | select(.name == "tilewright-render-ui") | .license) = "MIT AND OFL-1.1"' \
  "${updated}" >"${approved_font}"
run_check "${approved_font}"

for mutation in unrelated wrong-version forbidden-conjunct; do
  case "${mutation}" in
    unrelated) expression='(.packages[] | select(.name == "tilewright-render-ui") | .name) = "unrelated-font"' ;;
    wrong-version) expression='(.packages[] | select(.name == "tilewright-render-ui") | .version) = "0.6.0"' ;;
    forbidden-conjunct) expression='(.packages[] | select(.name == "tilewright-render-ui") | .license) = "MIT AND OFL-1.1 AND GPL-3.0-only"' ;;
  esac
  jq "${expression}" "${approved_font}" >"${temporary}/${mutation}.json"
  if run_check "${temporary}/${mutation}.json"; then
    echo "dependency policy accepted out-of-scope font license: ${mutation}" >&2
    exit 1
  fi
done

# Exercise policy schema failures in a private repository, never mutate policy
# in the caller's checkout while a concurrent validator may be using it.
mkdir -p "${temporary}/repository/tools" "${temporary}/repository/policy"
cp tools/check-dependencies.sh "${temporary}/repository/tools/"
git -C "${temporary}/repository" init --quiet
for mutation in wildcard duplicate missing-reason unknown-key empty-license; do
  case "${mutation}" in
    wildcard) expression='.package_license_exceptions[0].version = "*"' ;;
    duplicate) expression='.package_license_exceptions += [.package_license_exceptions[0]]' ;;
    missing-reason) expression='del(.package_license_exceptions[0].reason)' ;;
    unknown-key) expression='.package_license_exceptions[0].future = true' ;;
    empty-license) expression='.package_license_exceptions[0].allowed_spdx = []' ;;
  esac
  jq "${expression}" policy/dependencies.json >"${temporary}/repository/policy/dependencies.json"
  if (cd "${temporary}/repository" && run_check "${approved_font}"); then
    echo "dependency policy accepted invalid package license exception: ${mutation}" >&2
    exit 1
  fi
done
