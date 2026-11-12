# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.34.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.34.0/rigtop-v0.34.0-aarch64-apple-darwin.tar.gz"
    sha256 "07787b2a845a62ffc48ac777ae2ec11005e1effbce1e154da100bf1d0a630bae"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.34.0/rigtop-v0.34.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "82ec9fae655506c0fe3132957f7a666994064096c472a5c78c2d6f04587e3b93"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.34.0/rigtop-v0.34.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f2660d14d4259e63f23d268dc912a4c7659b03ccd7efa0882a3b5f2a527e2804"
    end
  end

  def install
    bin.install "rigtop"
    # The Linux aarch64 build has the terminal UI only.
    bin.install "rigtop-gui" if File.exist?("rigtop-gui")
  end

  def caveats
    <<~EOS
      Update rigtop with `brew upgrade rigtop`; its own update button
      only points there for a Homebrew install.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rigtop --version")
  end
end
