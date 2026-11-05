# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.28.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.28.0/rigtop-v0.28.0-aarch64-apple-darwin.tar.gz"
    sha256 "48844a25f5dd21e3b8e483b13d2a33cd1cce6d11fd9e336680a01739656358d0"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.28.0/rigtop-v0.28.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "17431c27c1dd0945890c318e36323b69af5e1cc14b97ef21ac9fa98c6d4de698"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.28.0/rigtop-v0.28.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "a4cc6765f02ec188fd2af3e7a76a41a7b73a25b5da7debe05263f2cfc119ed80"
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
