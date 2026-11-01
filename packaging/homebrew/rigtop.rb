# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.26.2"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.26.2/rigtop-v0.26.2-aarch64-apple-darwin.tar.gz"
    sha256 "0fdd204311beee4d5fdabdaea2bb957c341466b3b1a1f2197c35c2c51db1f264"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.26.2/rigtop-v0.26.2-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "7a41dc22af95cb96ff2772789aeee16ea7649511e1518300c0255d57503d74ed"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.26.2/rigtop-v0.26.2-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "bd30cdf54f1c8379a5fd3cb33845f6c1b6b9ebc0a8f569d24ff56c9dad85774b"
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
