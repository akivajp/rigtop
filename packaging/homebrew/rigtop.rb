# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.27.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.27.0/rigtop-v0.27.0-aarch64-apple-darwin.tar.gz"
    sha256 "4e47949aba02840078828452bf309a48d404fa2e8cf9a34f8a4abdd9e3e8ddac"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.27.0/rigtop-v0.27.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8a34a233ca8688df7db850a05cc7ec7f21679f8830740cc163da7c18c75b6b53"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.27.0/rigtop-v0.27.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1c2c9cffac9c47dcc3b92a56493c3433e370aec53b0f08b52a646cbeeba7dc95"
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
