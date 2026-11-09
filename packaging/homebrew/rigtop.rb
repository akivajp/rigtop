# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.32.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.32.0/rigtop-v0.32.0-aarch64-apple-darwin.tar.gz"
    sha256 "3705775a1d081b9b138b9497b63b7e60ae6311fb811585109b2379cd8b54b98d"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.32.0/rigtop-v0.32.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "1bdc5ddb7a7b51d6ef5817935e42d49264206bcf96452e831c28fa5d6ac3aa7e"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.32.0/rigtop-v0.32.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f6215926d1ebc10f2dfcde6919781d45935c0112ec741821407b55f2b3a1c80b"
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
