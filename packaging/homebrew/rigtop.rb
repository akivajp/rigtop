# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.26.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.26.0/rigtop-v0.26.0-aarch64-apple-darwin.tar.gz"
    sha256 "d10ee206c2e8413fc5a23646ba9d130653d713fec50c28c95150d36404936d9d"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.26.0/rigtop-v0.26.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "3ec41e476a159ae6169ed29d164be36bf7f10b59bbf8e43d22156d9cfffa438d"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.26.0/rigtop-v0.26.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "3721c87dc9febbf908e384a8b3b652107597d35c9cbdc1fd2c0ef87ef2d05a0d"
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
