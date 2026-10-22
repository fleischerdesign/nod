#!/usr/bin/env python3
"""A minimal nod provider (ADR-030).

Speaks JSON Lines over stdin/stdout. The whole protocol is four operations:
handshake, observe, plan, apply. Nothing here imports nod.

State lives in a JSON file so the example needs no API:

    settings = {"state": "/tmp/example-provider.json"}

Run it by hand:

    echo '{"protocol":1}' | ./nod-provider-example.py
"""

import json
import os
import sys

PROTOCOL = 1
KIND = "example.record"


def handshake():
    return {
        "provider": "example",
        "version": "0.1.0",
        "protocol": PROTOCOL,
        "capabilities": ["example.record"],
        "ownership": "marker",
        "settings_schema": {
            "type": "object",
            "required": ["state"],
            "properties": {
                "state": {
                    "type": "string",
                    "description": "Path to the JSON file holding the records",
                }
            },
        },
    }


def state_path(config):
    try:
        return config["state"]
    except KeyError:
        raise ValueError("config.state is required")


def load(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except FileNotFoundError:
        return {}


def save(path, data):
    temporary = path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(data, handle)
    os.replace(temporary, path)


def observe(config):
    data = load(state_path(config))
    return {
        "resources": [
            {"kind": KIND, "id": record_id, "attributes": attributes, "owned": True}
            for record_id, attributes in data.items()
        ],
        "conditions": [],
    }


def plan(config, desired):
    data = load(state_path(config))
    changes = []

    for want in desired:
        record_id = want["id"]
        attributes = want.get("attributes", {})
        if record_id not in data:
            changes.append(
                {"kind": "create", "owned": True, "destructive": False, "resource": want}
            )
        elif data[record_id] != attributes:
            changes.append(
                {"kind": "update", "owned": True, "destructive": False, "resource": want}
            )

    wanted_ids = {want["id"] for want in desired}
    for record_id, attributes in data.items():
        if record_id not in wanted_ids:
            changes.append(
                {
                    "kind": "delete",
                    "owned": True,
                    "destructive": True,
                    "resource": {"kind": KIND, "id": record_id, "attributes": attributes},
                }
            )

    return {"changes": changes, "conditions": []}


def apply(config, changes):
    path = state_path(config)
    data = load(path)
    applied = []

    for change in changes:
        record_id = change["resource"]["id"]
        if change["kind"] == "delete":
            data.pop(record_id, None)
        else:
            data[record_id] = change["resource"].get("attributes", {})
        applied.append({"id": record_id, "ok": True})

    save(path, data)
    return {"applied": applied, "conditions": []}


def handle(request):
    operation = request.get("op")
    config = request.get("config", {})

    if operation is None:
        return handshake()
    if operation == "observe":
        return observe(config)
    if operation == "plan":
        return plan(config, request.get("desired", []))
    if operation == "apply":
        return apply(config, request.get("changes", []))
    return {"error": {"kind": "protocol", "message": f"unknown operation {operation}"}}


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as error:
            response = {"error": {"kind": "protocol", "message": f"invalid json: {error}"}}
        else:
            try:
                response = handle(request)
            except ValueError as error:
                response = {"error": {"kind": "config", "message": str(error)}}
        stdout_line = json.dumps(response)
        sys.stdout.write(stdout_line + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
