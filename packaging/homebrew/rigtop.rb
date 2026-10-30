# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.20.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.20.0/rigtop-v0.20.0-aarch64-apple-darwin.tar.gz"
    sha256 "cca3b30c012743887719cbe960768d9ee5ad930e2f53714f9b0d22f8a2aeeb47"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.20.0/rigtop-v0.20.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ed92de65f3c40d3e47c3b9be6d4b450dae8103b9976e30032ddefadbea530d80"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.20.0/rigtop-v0.20.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c99edeb89413e3d38f156833c698b560e138fe40f7850663ad734e273841d59b"
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
