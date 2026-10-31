# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.23.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.23.0/rigtop-v0.23.0-aarch64-apple-darwin.tar.gz"
    sha256 "bd2bdf50275ff2c6ec6d70383b29641dd6ee18abf6e5eca0cf1588e351513d1f"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.23.0/rigtop-v0.23.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8dd821f0067bb8134e01a6151af65b57e7a5de503bee94450f96ee3886742a59"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.23.0/rigtop-v0.23.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "280bc6598d36873870bb848763b739cd12c7af943633e884c485c36f5c9c41cb"
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
