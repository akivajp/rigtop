# Homebrew formula for rigtop, installed from the release binaries.
# Kept here and copied to https://github.com/akivajp/homebrew-tap for each
# release (version, URLs and sha256 from SHA256SUMS).
class Rigtop < Formula
  desc "System monitor for workstations, laptops and NAS (terminal UI and native GUI)"
  homepage "https://github.com/akivajp/rigtop"
  version "0.35.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    # Release builds exist for Apple Silicon only.
    depends_on arch: :arm64

    url "https://github.com/akivajp/rigtop/releases/download/v0.35.0/rigtop-v0.35.0-aarch64-apple-darwin.tar.gz"
    sha256 "59a533fdf4f501b61b320646b3d14754b23ea00d4bbd8c8b951f52566157f66a"
  end

  on_linux do
    on_intel do
      url "https://github.com/akivajp/rigtop/releases/download/v0.35.0/rigtop-v0.35.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "f45a11401e3bddd6a7cee8671b3740b3aae4402b237223669e240cb4178b80cf"
    end
    on_arm do
      url "https://github.com/akivajp/rigtop/releases/download/v0.35.0/rigtop-v0.35.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "523504ce8a26b09271fe0414e0384f04de58522671f32b6d1bf3c55f1a72a927"
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
