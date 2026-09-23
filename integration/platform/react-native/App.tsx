import { useEffect, useState } from "react";
import { SafeAreaView, ScrollView, Text } from "react-native";
import { runSmoke } from "./smoke";

export default function App() {
  const [result, setResult] = useState("AXTON React Native integration");
  useEffect(() => {
    void runSmoke(setResult);
  }, []);
  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: "#fff" }}>
      <ScrollView contentContainerStyle={{ padding: 24 }}>
        <Text style={{ fontSize: 22, fontWeight: "600", marginBottom: 20 }}>
          AXTON runtime check
        </Text>
        <Text selectable style={{ fontSize: 16, lineHeight: 25 }}>
          {result}
        </Text>
      </ScrollView>
    </SafeAreaView>
  );
}
