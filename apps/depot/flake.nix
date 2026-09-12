# UNVERIFIED: written without nix available. Pattern: buildRustPackage with
# the committed Cargo.lock; no C dependencies in the default feature set.
{
  description = "depot — offline mirror of crates, RPMs, a Nix cache and Rust toolchains";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forAll (pkgs: rec {
        depot = pkgs.rustPlatform.buildRustPackage {
          pname = "depot";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          # shared kits are path deps outside this directory; a flake built
          # from the monorepo root would set src = ../.. and sourceRoot.
          doCheck = true;
          meta.mainProgram = "depot";
        };
        default = depot;
      });

      nixosModules.depot = { config, lib, pkgs, ... }:
        let cfg = config.services.depot; in {
          options.services.depot = {
            enable = lib.mkEnableOption "depot offline mirror";
            package = lib.mkOption { type = lib.types.package; default = self.packages.${pkgs.system}.depot; };
            openFirewall = lib.mkOption { type = lib.types.bool; default = false; };
          };
          config = lib.mkIf cfg.enable {
            systemd.services.depot = {
              description = "depot offline mirror";
              wantedBy = [ "multi-user.target" ];
              after = [ "network-online.target" ];
              serviceConfig = {
                ExecStartPre = "${cfg.package}/bin/depot init";
                ExecStart = "${cfg.package}/bin/depot serve";
                DynamicUser = true;
                StateDirectory = "depot";
                Restart = "on-failure";
                ProtectSystem = "strict";
                ProtectHome = true;
                PrivateTmp = true;
                NoNewPrivileges = true;
                RestrictAddressFamilies = [ "AF_INET" "AF_INET6" ];
                CapabilityBoundingSet = "";
              };
            };
            networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [ 8000 ];
          };
        };
    };
}
