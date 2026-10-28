# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.15.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.15.0/rigtop-v0.15.0-aarch64-apple-darwin.tar.gz"
    sha256 "ab500addb06743595be4120787bcc0d91a8b38ac9260d09b52d3f425421799b8"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.15.0/rigtop-v0.15.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "710d42188a1efaf3b6e22895c633e2e380a8b5f69ffc3fe48713eb446398f5d8"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.15.0/rigtop-v0.15.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "31ae608397f598234f8338e53a8d1fd7d8c26c0991cfc3ab324ee604762a127d"
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
