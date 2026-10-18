#!/usr/bin/env bash
set -euo pipefail

repository=$(git rev-parse --show-toplevel)
cd "${repository}"

jq -e '
  def nonempty_string: type == "string" and length > 0;

  .schema_version == 2
  and (.allowed_spdx
    | type == "array" and length > 0
      and all(.[]; nonempty_string) and length == (unique | length))
  and (.allowed_spdx_exceptions
    | type == "array"
      and all(.[]; nonempty_string) and length == (unique | length))
  and (.allowed_registry_sources
    | type == "array" and length > 0
      and all(.[]; nonempty_string) and length == (unique | length))
  and (.direct_dependencies | type == "array" and length > 0)
  and all(.direct_dependencies[];
    type == "object" and (.name | nonempty_string))
  and all(.direct_dependencies[];
    type == "object" and (.purpose | nonempty_string))
  and (.direct_dependencies | map(.name) | length == (unique | length))
  and (.owner | nonempty_string) and (.review_cadence | nonempty_string)
  and (.eol_response | nonempty_string) and (.validation | nonempty_string)
' policy/dependencies.json >/dev/null

metadata=$(mktemp /tmp/tilewright-renderer-metadata.XXXXXX)
trap 'rm -f -- "${metadata}"' EXIT
cargo metadata --locked --offline --all-features --format-version 1 >"${metadata}"

jq -e --slurpfile policy policy/dependencies.json '
  def evaluate_license($allowed; $exceptions):
    def reduce_with:
      index("WITH") as $operator
      | if $operator == null then .
        elif $operator == 0 or $operator + 1 >= length then error("invalid WITH")
        else
          .[($operator - 1)] as $left
          | .[($operator + 1)] as $right
          | if ($left | type) != "string"
              or ($right | type) != "string"
              or (["AND", "OR", "WITH", "(", ")", "/"]
                | index($left)) != null
              or (["AND", "OR", "WITH", "(", ")", "/"]
                | index($right)) != null
            then error("invalid WITH operands")
            else
              (.[0:($operator - 1)]
                + [
                    (($allowed | index($left) != null)
                      and ($exceptions | index($right) != null))
                  ]
                + .[($operator + 2):])
              | reduce_with
            end
        end;
    def identifiers_to_booleans:
      map(if type == "boolean" then .
          elif . == "AND" or . == "OR" then .
          elif type != "string" or . == "WITH"
          then error("invalid license token: \(.)")
          else . as $license | $allowed | index($license) != null
          end);
    def reduce_and:
      index("AND") as $operator
      | if $operator == null then .
        elif $operator == 0 or $operator + 1 >= length then error("invalid AND")
        elif (.[($operator - 1)] | type) != "boolean"
          or (.[($operator + 1)] | type) != "boolean"
        then error("invalid AND operands")
        else
          (.[0:($operator - 1)]
            + [(.[($operator - 1)] and .[$operator + 1])]
            + .[($operator + 2):])
          | reduce_and
        end;
    def reduce_or:
      index("OR") as $operator
      | if $operator == null then .
        elif $operator == 0 or $operator + 1 >= length then error("invalid OR")
        elif (.[($operator - 1)] | type) != "boolean"
          or (.[($operator + 1)] | type) != "boolean"
        then error("invalid OR operands")
        else
          (.[0:($operator - 1)]
            + [(.[($operator - 1)] or .[$operator + 1])]
            + .[($operator + 2):])
          | reduce_or
        end;
    def evaluate_parentheses:
      index(")") as $close
      | if $close == null then
          if index("(") == null then . else error("unclosed (") end
        else
          . as $tokens
          | ([range(0; $close) | select($tokens[.] == "(")] | last) as $open
          | if $open == null then error("unexpected )")
            else
              ($tokens[($open + 1):$close]
                | evaluate_license($allowed; $exceptions)) as $inner
              | ($tokens[0:$open] + [$inner] + $tokens[($close + 1):])
              | evaluate_parentheses
            end
        end;
    map(if . == "/" then "OR" else . end)
    | evaluate_parentheses
    | reduce_with
    | identifiers_to_booleans
    | reduce_and
    | reduce_or
    | if length == 1 and (.[0] | type == "boolean") then .[0]
      else error("invalid license expression")
      end;

  . as $metadata
  | ($metadata.workspace_members | unique) as $workspace_members
  | [
      $metadata.packages[]
      | select(.id as $id | $workspace_members | index($id))
      | .dependencies[]
      | select(.source != null)
      | .name
    ] | unique | sort as $direct_names
  | ($policy[0].direct_dependencies | map(.name) | unique | sort) as $policy_names
  | $direct_names == $policy_names
    and all($metadata.packages[];
      (.license // "") as $expression
      | [$expression
          | scan("\\(|\\)|AND|OR|WITH|/|[A-Za-z0-9][A-Za-z0-9.+-]*")]
        as $tokens
      | ($expression | gsub("\\s"; "")) == ($tokens | join(""))
        and ($tokens
          | evaluate_license(
              $policy[0].allowed_spdx;
              $policy[0].allowed_spdx_exceptions)))
    and all($metadata.packages[];
      if (.id as $id | $workspace_members | index($id))
      then .source == null
      else .source as $source | $policy[0].allowed_registry_sources | index($source)
      end)
' "${metadata}" >/dev/null
