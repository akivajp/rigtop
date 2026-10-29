# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.17.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.17.0/rigtop-v0.17.0-aarch64-apple-darwin.tar.gz"
    sha256 "18deb0c4dc9c5bb5c317d2f41423e040912edb5d27aafcadb74ed3655290d3bb"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.17.0/rigtop-v0.17.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ff3032bdcff6bf5b8428317b7f89f3d9e8f6247e4eef46c374b71e0375d9ec25"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.17.0/rigtop-v0.17.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1d7b6f0eaa3d9f3771bb5729d8ae885d69bc11d695f4ce5ac7ec6be1506a0ab0"
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
