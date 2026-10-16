#!/bin/sh
# Wonder path: grow industry toward the dyson ring. Speaks CLI JSON only.
read -r _ || true
printf '%s\n' '{"cmd":["build","home","metal_mine"]}'
printf '%s\n' '{"cmd":["build","home","crystal_mine"]}'
printf '%s\n' '{"cmd":["build","home","solar_plant"]}'
printf '%s\n' '{"cmd":["build","home","research_lab"]}'
