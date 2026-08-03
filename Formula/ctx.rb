# Template rendered by .github/workflows/release.yml's `formula` job, which
# fills the placeholder tokens below (double-underscore delimited) from the
# same release build that produces the ctx-v{version}-{target}.tar.gz
# archives (see modules/cli/Cargo.toml's [package.metadata.binstall] for the
# naming contract). The rendered copy is attached to the GitHub release and
# pushed to the tap by the `tap` job; this file stays unrendered on main.
class Ctx < Formula
  desc "ctx.traits reference CLI and runtime"
  homepage "https://github.com/__REPO__"
  version "__VERSION__"
  license "Apache-2.0"

  on_macos do
    on_intel do
      url "https://github.com/__REPO__/releases/download/v__VERSION__/ctx-v__VERSION__-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_X86_64_APPLE_DARWIN__"
    end
    on_arm do
      url "https://github.com/__REPO__/releases/download/v__VERSION__/ctx-v__VERSION__-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_AARCH64_APPLE_DARWIN__"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/__REPO__/releases/download/v__VERSION__/ctx-v__VERSION__-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "__SHA256_X86_64_UNKNOWN_LINUX_GNU__"
    end
    on_arm do
      url "https://github.com/__REPO__/releases/download/v__VERSION__/ctx-v__VERSION__-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "__SHA256_AARCH64_UNKNOWN_LINUX_GNU__"
    end
  end

  def install
    bin.install "ctx"
  end

  test do
    system "#{bin}/ctx", "--version"
  end
end
