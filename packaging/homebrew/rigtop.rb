# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.36.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.36.1/rigtop-v0.36.1-aarch64-apple-darwin.tar.gz"
    sha256 "52f9984a95d914a41b5b81ecacaa3dbdc6d8666522b73f329a4a270c5ce494ca"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.36.1/rigtop-v0.36.1-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "968e105c7dce0e9b3b968a2e7a31ce9e83aed7edd786368b2a800001d7a22f71"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.36.1/rigtop-v0.36.1-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "3de8e7fe2f28a34141d9a67d3dda5d43e21906c6c84de7643b429ae886b9f1be"
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
