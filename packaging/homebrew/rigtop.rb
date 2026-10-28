# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.16.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.16.0/rigtop-v0.16.0-aarch64-apple-darwin.tar.gz"
    sha256 "11f78215f90e5a62f7f6ee2e7c07da2e22b045d8de20c48ef46bc354166888a1"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.16.0/rigtop-v0.16.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "c7ed42cc7dd09cbee71e42f2ee7e933d19e1967336a3ef71cd744a540be56f88"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.16.0/rigtop-v0.16.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "df04235342289f96c2a1785b08e82bc63552b2b33f181436c40541f16504c629"
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
