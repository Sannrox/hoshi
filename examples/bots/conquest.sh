#!/bin/sh
# Conquest path: yards first, then mines. Speaks CLI JSON only.
read -r _ || true
printf '%s\n' '{"cmd":["build","home","metal_mine"]}'
printf '%s\n' '{"cmd":["build","home","solar_plant"]}'
printf '%s\n' '{"cmd":["build","home","robotics_factory"]}'
printf '%s\n' '{"cmd":["build","home","shipyard"]}'
