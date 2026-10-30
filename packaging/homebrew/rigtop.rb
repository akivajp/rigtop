# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.19.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.19.0/rigtop-v0.19.0-aarch64-apple-darwin.tar.gz"
    sha256 "0b5c0f5cae19c13e63094ea0a33d178b6dd9ca63844591673b1619f87ef8782f"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.19.0/rigtop-v0.19.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "65ac2dcf1a6fffb89a524e2c86af9278168d4f7608324f73ce7d8f3a8109e821"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.19.0/rigtop-v0.19.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f2a557df42f1a2f1e82441ce87dc82d41f20c007fae59317db77eeab07ca2b13"
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
