#!/usr/bin/env python3
"""Generate the web viewer's layer table from a KLayout layer-properties (.lyp) file.

    scripts/gen-layer-stack.sh   # reads the PDK from the Docker volume and runs this

Output: JSON with one entry per (layer, datatype): name, colours, KLayout dither
pattern, and a draw order. Process layers (wells up to the top metals) get the
order of the physical stack; everything else is drawn below them and hidden by
default.
"""
import json
import sys
import xml.etree.ElementTree as ET

# Physical stack, bottom to top. Purposes of these layers that are drawn by default.
STACK = [
    "NWell", "Activ", "pSD", "nSD", "GatPoly", "Cont", "Metal1", "Via1", "Metal2", "Via2",
    "Metal3", "Via3", "Metal4", "Via4", "Metal5", "TopVia1", "TopMetal1", "TopVia2", "TopMetal2",
    "Passiv", "prBoundary",
]
DEFAULT_PURPOSES = {"drawing", "pin"}


def main(lyp_path: str, pdk: str, out_path: str) -> None:
    root = ET.parse(lyp_path).getroot()
    layers = {}
    for p in root.iter("properties"):
        name, source = p.findtext("name"), p.findtext("source")
        if not name or not source or "/" not in source:
            continue
        try:
            layer, datatype = (int(x) for x in source.split("@")[0].strip().split("/"))
        except ValueError:
            continue
        base, _, purpose = name.partition(".")
        rank = STACK.index(base) if base in STACK else -1
        key = f"{layer}/{datatype}"
        if key in layers:
            continue
        layers[key] = {
            "name": name,
            "layer": layer,
            "datatype": datatype,
            "fill": p.findtext("fill-color") or "#808080",
            "frame": p.findtext("frame-color") or p.findtext("fill-color") or "#808080",
            "dither": p.findtext("dither-pattern") or "C0",
            # Draw order: higher is drawn later (on top). Pins sit just above their layer's drawing.
            "order": (rank + 1) * 10 + (1 if purpose == "pin" else 0),
            # The placement boundary is the die outline: always useful, whatever its purpose.
            "visible": rank >= 0 and (purpose in DEFAULT_PURPOSES or base == "prBoundary"),
        }
    data = {
        "pdk": pdk,
        "source": "KLayout layer properties (sg13g2.lyp) from the IHP Open PDK, Apache-2.0, (c) IHP PDK Authors",
        "layers": sorted(layers.values(), key=lambda l: (l["layer"], l["datatype"])),
    }
    with open(out_path, "w") as f:
        json.dump(data, f, indent=1)
        f.write("\n")
    print(f"wrote {len(layers)} layers to {out_path}")


if __name__ == "__main__":
    main(*sys.argv[1:4])
