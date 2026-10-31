# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.24.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.24.0/rigtop-v0.24.0-aarch64-apple-darwin.tar.gz"
    sha256 "1744bca0cfebde48fa56cf2dbd44c2d18e62d5991d3228dbf8465c1b3b48979f"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.24.0/rigtop-v0.24.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "cf72f1afea64b70a8b3ac9d42db6155caa8c40bab2d64615422b24ae37ca939e"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.24.0/rigtop-v0.24.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "96a972b7df450e8a678893560b86a8d38229fb9c13a040106782ff890a0c7bc9"
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
