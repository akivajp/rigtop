# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.36.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.36.0/rigtop-v0.36.0-aarch64-apple-darwin.tar.gz"
    sha256 "de7c4781e2f2cd81aa29a78406a07917e40bfd5690155f26fc473020cfc3aa25"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.36.0/rigtop-v0.36.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d24689bc48f33ec483f5617a9034eda7a23cd867aa3ab7fbdcb8eeeff3f009b1"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.36.0/rigtop-v0.36.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "cc288379dc0a03f6c589bbf32aa95c528ecbdce535925083bf3df2857e15d5a8"
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
