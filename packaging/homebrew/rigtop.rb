# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.33.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.33.0/rigtop-v0.33.0-aarch64-apple-darwin.tar.gz"
    sha256 "930cc978653fc73645f29d87bfec17bff9916d10bc12a45920fb0dbe97e7c1c9"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.33.0/rigtop-v0.33.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "dafe0733f4c81ec30fb6b5b0a42089e778189740c0acc1b46698f0accf284bc0"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.33.0/rigtop-v0.33.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c993b3df4d155cc3efa39277b3d06b0dcde135e7fa86f660002459044369a5e9"
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
