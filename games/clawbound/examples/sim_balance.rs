use clawbound_rs::sim::{Game, Outcome, Mode};
use clawbound_rs::hunter::Kind;
fn run(k:Kind,name:&str,mode:Mode,mlabel:&str){
  let mut g=Game::new_match(k,mode);
  let mut steps=0;
  while g.outcome.is_none() && steps < 95*30 { g.step(1.0/30.0); steps+=1; }
  let out = match g.outcome { Some(Outcome::CatsWin)=>"CATS WIN", Some(Outcome::HunterWins)=>"HUNTER WINS", None=>"timeout" };
  eprintln!("{:7} [{:10}] {:11}  escaped {}  caught {}", name, mlabel, out, g.escaped, g.caught);
}
fn main(){
  for (k,n) in [(Kind::Fox,"Fox"),(Kind::Dog,"Dog"),(Kind::Badger,"Badger"),(Kind::Hawk,"Hawk"),(Kind::Snake,"Snake"),(Kind::Shadow,"Shadow")]{
    run(k,n,Mode::Rendezvous,"Rendezvous"); run(k,n,Mode::Survive,"Survive");
  }
}
