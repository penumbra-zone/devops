use kube::CustomResourceExt;
use penumbra_operator::crd::PenumbraNode;

fn main() {
    print!("{}", serde_yaml::to_string(&PenumbraNode::crd()).unwrap())
}
