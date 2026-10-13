#!/usr/bin/env python3
"""Sample arena bot: queue the next home mines. Speaks CLI JSON only."""

import json
import sys

sys.stdin.readline()
for building in ("metal_mine", "crystal_mine", "solar_plant"):
    print(json.dumps({"cmd": ["build", "home", building]}))
