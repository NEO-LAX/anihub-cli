{
  description = "Unofficial terminal client for browsing and watching anime from AniHub";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = {self, nixpkgs}: let
    supportedSystems = [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ];
    forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
  in {
    packages = forAllSystems (system: let
      pkgs = nixpkgs.legacyPackages.${system};
      anihub-cli = pkgs.rustPlatform.buildRustPackage {
        pname = "anihub-cli";
        version = "0.7.2";
        src = pkgs.lib.cleanSource ./.;
        cargoLock.lockFile = ./Cargo.lock;

        nativeBuildInputs = [
          pkgs.cacert
          pkgs.makeWrapper
        ];

        SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
        NIX_SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";

        postInstall = ''
          wrapProgram "$out/bin/anihub-cli" \
            --prefix PATH : ${pkgs.lib.makeBinPath [pkgs.mpv]} \
            --set SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt" \
            --set NIX_SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
        '';

        meta = {
          description = "Unofficial terminal client for AniHub";
          homepage = "https://github.com/NEO-LAX/anihub-cli";
          license = pkgs.lib.licenses.mit;
          mainProgram = "anihub-cli";
          platforms = supportedSystems;
        };
      };
    in {
      default = anihub-cli;
      inherit anihub-cli;
    });

    apps = forAllSystems (system: {
      default = {
        type = "app";
        program = "${self.packages.${system}.default}/bin/anihub-cli";
      };
    });

    checks = forAllSystems (system: {
      inherit (self.packages.${system}) anihub-cli;
    });
  };
}
