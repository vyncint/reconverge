#!/usr/bin/env python3
"""Validate reconverge artifacts against the JSON Schemas in `schemas/`.

A small, self-contained Draft 2020-12 subset rather than the `jsonschema`
package: adding a dependency to a CI job is a stop-and-ask event under
CONTRIBUTING §0.4, and the schemas here use only the keywords implemented
below. A keyword they grow that is not implemented fails loudly rather
than passing silently -- a validator that quietly ignores a constraint
reports success for the wrong reason, which is the failure mode that put
us here.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# Keywords this validator understands. A schema using anything else is a
# hard error: a validator that quietly ignores a constraint is worse than
# no validator, because the gate then reports success for the wrong reason.
SUPPORTED = {
    "$schema", "$id", "$defs", "$ref", "title", "description",
    "type", "const", "enum", "required", "properties",
    "items", "minItems", "maxItems",
    "minimum", "maximum", "multipleOf", "pattern", "minLength",
}

TYPES = {
    "object": dict,
    "array": list,
    "string": str,
    "integer": int,
    "number": (int, float),
    "boolean": bool,
    "null": type(None),
}


def resolve(root: dict, ref: str) -> dict:
    if not ref.startswith("#/"):
        raise SystemExit(f"only local $ref is supported, got {ref!r}")
    node = root
    for part in ref[2:].split("/"):
        node = node[part]
    return node


def validate(value, schema: dict, root: dict, path: str, errors: list[str]) -> None:
    unsupported = set(schema) - SUPPORTED
    if unsupported:
        raise SystemExit(
            f"{path}: schema uses keywords this validator does not implement: "
            f"{sorted(unsupported)} — teach scripts/validate-schema.py or the "
            f"gate is lying"
        )

    if "$ref" in schema:
        validate(value, resolve(root, schema["$ref"]), root, path, errors)
        return

    if "type" in schema:
        expected = schema["type"]
        wanted = TYPES[expected]
        # JSON has no integer type; `True` is an `int` in Python and is not.
        if isinstance(value, bool) and expected in ("integer", "number"):
            errors.append(f"{path}: {value!r} is not {expected}")
            return
        if not isinstance(value, wanted):
            errors.append(f"{path}: {value!r} is not {expected}")
            return

    if "const" in schema and value != schema["const"]:
        errors.append(f"{path}: {value!r} != const {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{path}: {value!r} not in {schema['enum']}")
    if "minLength" in schema and isinstance(value, str):
        if len(value) < schema["minLength"]:
            errors.append(f"{path}: {value!r} is shorter than {schema['minLength']}")
    if "pattern" in schema and isinstance(value, str):
        if not re.search(schema["pattern"], value):
            errors.append(f"{path}: {value!r} does not match {schema['pattern']!r}")
    for key, op, name in (
        ("minimum", lambda v, b: v >= b, ">="),
        ("maximum", lambda v, b: v <= b, "<="),
    ):
        if key in schema and isinstance(value, (int, float)):
            if not op(value, schema[key]):
                errors.append(f"{path}: {value} is not {name} {schema[key]}")
    if "multipleOf" in schema and isinstance(value, (int, float)):
        if value % schema["multipleOf"] != 0:
            errors.append(f"{path}: {value} is not a multiple of {schema['multipleOf']}")

    if isinstance(value, dict):
        for name in schema.get("required", []):
            if name not in value:
                errors.append(f"{path}: missing required field {name!r}")
        for name, sub in schema.get("properties", {}).items():
            if name in value:
                validate(value[name], sub, root, f"{path}/{name}", errors)

    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            errors.append(f"{path}: {len(value)} items, minimum {schema['minItems']}")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            errors.append(f"{path}: {len(value)} items, maximum {schema['maxItems']}")
        if "items" in schema:
            for i, item in enumerate(value):
                validate(item, schema["items"], root, f"{path}/{i}", errors)


def check(document, schemas: dict[str, dict], label: str) -> list[str]:
    """Validate one document against whichever schema it declares."""
    declared = document.get("schema") if isinstance(document, dict) else None
    if declared not in schemas:
        return [f"{label}: declares schema {declared!r}, which is not in schemas/"]
    errors: list[str] = []
    validate(document, schemas[declared], schemas[declared], label, errors)
    return errors


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--schema-dir", type=Path, required=True)
    ap.add_argument("--fixtures", type=Path, required=True)
    ap.add_argument("--emitted", type=Path, required=True)
    ap.add_argument("--jsonl", type=Path, required=True)
    args = ap.parse_args()

    schemas = {}
    for path in sorted(args.schema_dir.glob("*.json")):
        schema = json.loads(path.read_text())
        schemas[schema["title"]] = schema
    if not schemas:
        print("no schemas found", file=sys.stderr)
        return 1

    errors: list[str] = []
    checked = 0

    for path in sorted(args.fixtures.rglob("*.json")):
        document = json.loads(path.read_text())
        if not isinstance(document, dict) or "schema" not in document:
            continue  # a scenario input, not an artifact
        errors += check(document, schemas, str(path.relative_to(args.fixtures.parent)))
        checked += 1

    # What a real run wrote. This is the corpus the fixtures cannot stand in
    # for: they are hand-picked, and the multi-warp witness that broke the
    # published bound was never one of them.
    emitted_witnesses = 0
    for path in sorted(args.emitted.glob("*.json")):
        document = json.loads(path.read_text())
        if not isinstance(document, dict) or "schema" not in document:
            continue
        errors += check(document, schemas, str(path.name))
        checked += 1
        if document["schema"] == "witness.v1" and document.get("lanes", 32) > 32:
            emitted_witnesses += 1

    for i, line in enumerate(args.jsonl.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        errors += check(json.loads(line), schemas, f"{args.jsonl.name}:{i}")
        checked += 1

    if emitted_witnesses == 0:
        errors.append(
            "the end-to-end run emitted no witness wider than one warp; without "
            "one, this gate never sees the shape witness.v1 used to reject"
        )

    if errors:
        print(f"schema validation FAILED ({len(errors)} of {checked} documents):",
              file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1
    print(f"{checked} artifacts validate against schemas/")
    return 0


if __name__ == "__main__":
    sys.exit(main())
