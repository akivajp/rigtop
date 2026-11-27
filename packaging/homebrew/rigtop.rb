# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.37.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.37.0/rigtop-v0.37.0-aarch64-apple-darwin.tar.gz"
    sha256 "45e61aa08a669dff395f9db8c699dfecd6cfc225b070d1512c8ac245f0420e51"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.37.0/rigtop-v0.37.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "bb992a7dbfd17f16b2edacef4391cca0037de8f73bd435aa69ff19696673382d"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.37.0/rigtop-v0.37.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "7e9675ea47e4362dab4d14f8a78c99fa327db939eda7c49c91348d88a41735bd"
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
