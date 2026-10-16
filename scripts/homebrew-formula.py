"""Write Formula/hoshi.rb for a tagged GitHub Release."""

import hashlib
import re
import sys
from pathlib import Path


ARTEFACTS = {
    "macos_arm": "hoshi-aarch64-apple-darwin",
    "macos_intel": "hoshi-x86_64-apple-darwin",
    "linux_arm": "hoshi-aarch64-unknown-linux-gnu",
    "linux_intel": "hoshi-x86_64-unknown-linux-gnu",
    "windows": "hoshi-x86_64-pc-windows-msvc.exe",
}

TEMPLATE = """\
class Hoshi < Formula
  desc "Idle space-strategy game for the terminal"
  homepage "https://github.com/Sannrox/hoshi"
  version "%(version)s"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "%(base)s/%(macos_arm)s", using: :nounzip
      sha256 "%(macos_arm_sha)s"
    end
    on_intel do
      url "%(base)s/%(macos_intel)s", using: :nounzip
      sha256 "%(macos_intel_sha)s"
    end
  end

  on_linux do
    on_arm do
      url "%(base)s/%(linux_arm)s", using: :nounzip
      sha256 "%(linux_arm_sha)s"
    end
    on_intel do
      url "%(base)s/%(linux_intel)s", using: :nounzip
      sha256 "%(linux_intel_sha)s"
    end
  end

  def install
    bin.install Dir["hoshi*"].first => "hoshi"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hoshi --version")
  end
end
"""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: homebrew-formula.py vX.Y.Z DIST_DIR", file=sys.stderr)
        return 2
    tag, dist_arg = sys.argv[1], sys.argv[2]
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        print("homebrew-formula: tag must be vX.Y.Z", file=sys.stderr)
        return 2
    version = tag[1:]
    dist = Path(dist_arg)
    values = {
        "version": version,
        "base": "https://github.com/Sannrox/hoshi/releases/download/v" + version,
    }
    for key, name in ARTEFACTS.items():
        path = dist / name
        if not path.is_file():
            print("homebrew-formula: missing " + str(path), file=sys.stderr)
            return 1
        values[key] = name
        values[key + "_sha"] = sha256(path)
    out = dist / "hoshi.rb"
    out.write_text(TEMPLATE % values)
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
