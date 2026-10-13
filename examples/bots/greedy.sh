#!/bin/sh
# Sample arena bot: queue the next home mines. Speaks CLI JSON only.
read -r _ || true
printf '%s\n' '{"cmd":["build","home","metal_mine"]}'
printf '%s\n' '{"cmd":["build","home","crystal_mine"]}'
printf '%s\n' '{"cmd":["build","home","solar_plant"]}'
